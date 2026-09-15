# Circumventing the two measured ceilings

Where the walls are (measured, `findings/implementation.md` round 10):

- **Clone/fetch**: `Error::Budget` → 413 somewhere in 110k–263k objects.
  Spend = R2 range reads only — sqlite `q()` is free. A full clone's budget
  goes to (a) commit/tree object reads during walk+expansion, and (b) the
  planned `set.reads` pack reads that build the response. `blob:none` doesn't
  help — the walk reads dominate.
- **Import**: a single commit can introduce more objects than one request's
  ingest budget (TypeScript: commit `6d44e05`, ~222k objects). Staged pushes
  cannot split below one commit.

## Clone ceiling

### C1 — `packfile-uris` offload (the structural fix)

The protocol has this built in (`Documentation/technical/packfile-uri.adoc`):
server advertises `packfile-uris`; an opted-in client sends
`packfile-uris http,https` as a fetch arg; the server may then emit a
`packfile-uris` section of URIs *before* the `packfile` section, and the client
downloads + indexes those packs directly. URIs can cover any objects, not just
blobs.

- **Caveat 1**: `fetch.uriprotocols` defaults to empty — stock `git clone`
  won't ask for it; the user needs `-c fetch.uriprotocols=https`. So this is
  an informed-client path, with the inline packfile staying the default.
- **Caveat 2**: URI fetches carry no extra auth headers (explicit "future
  work" upstream). The URI must self-authenticate: a signed URL
  (`GET /packs/<key>?exp=…&sig=hmac(key,exp,repo)`) on an edge route that
  streams `bucket.get(key).body` — ~1 subrequest per GET regardless of pack
  size. A public-bucket/custom-domain variant works for public repos.
- **When can we emit a URI?** Whenever the send set is covered by pack(s)
  already in R2. The clean case: after `gc_consolidate`, one live pack covers
  all reachable objects → a full clone (`want=tip`, no haves/shallow/filter)
  is *exactly* that pack. Clone spend drops ~9,000 → ~10.
- A `clone-pack` job could materialize a canonical pack (`tip → all
  reachable`) into R2 on push/GC, so even mid-cycle clones get a URI instead
  of a generated stream.

### C2 — verbatim consolidated-pack fast path (cheap, helps default clients)

Even without `packfile-uris`: if a single live pack covers the full-clone send
set, stream it verbatim as the `packfile` section instead of per-object
`read_entries`. One sequential R2 read streams the whole pack (~1 subrequest
per `get`, body doesn't charge per chunk) — the clone wall lifts for any
recently-GC'd repo regardless of client flags. Gate to the plain-clone shape;
superset objects are legal (git index-packs extras, connectivity check still
passes) but must not violate shallow/filter contracts.

*Premise confirmed live*: after import + GC quiet window, `facebook/react`
settles to `packs_live: 1` covering all 263k objects — the fast path would
serve its clone in tens of subrequests.

### C3 — client-side workarounds today: none found

Tested on the live react repo (263k objects): `--deepen=10000` still ships
~the whole send set per request (413); `tree:0` is rejected (unsupported
filter); `blob:none` dies on the walk itself. Until C1/C2 land, repos past the
wall are import-only. Worth documenting as the standing answer.

## Import ceiling

### I1 — server-side import job (ROADMAP #4b, the structural fix)

`POST /_admin/import {r2_key}`: client uploads a pack or bundle to R2 staging
(chunked `/_admin/import-part` POSTs under the 100 MB cap, or S3 multipart),
then a resumable `import_pack` job ingests across alarm invocations — each
alarm gets a fresh request budget, which is exactly the `purge_repo` model
already proven in production. 945k objects ≈ ~20 alarm slices. Fixes
TypeScript-class imports outright.

### I2 — synthetic-ref object seeding (pure client-side, zero server changes)

For a single oversize commit: fabricate synthetic commits whose trees each
contain a *subset* of the giant commit's tree (`git mktree` + `commit-tree`),
push them to `refs/stage/1…N` in capped slices, then push the real branch —
the client sees the staged objects in the remote advertisement and sends only
the delta (commit + top-level trees). Delete the staging refs once the real
history covers them. A `--seed-subtrees` mode in `tools/git-edge-import.sh`.
Hacky but implementable today; validates the "pre-seed then dedup" trick.

## Suggested order

1. **C2** (verbatim pack fast path) — smallest diff, biggest coverage: helps
   every client, no opt-in needed. Measure on react: post-GC consolidated pack
   exists → clone should drop to tens of subrequests.
2. **C1** (packfile-uris) — on top of C2's coverage test; makes bandwidth
   leave the Worker entirely and is the artifact-fs-friendly shape.
3. **I1** (import job) — heavier; I2 is a cheap interim for the CLI.
4. **I2** only if I1 slips — real but weird; I1 is the honest fix.
