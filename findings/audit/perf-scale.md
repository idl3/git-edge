# Perf & scale audit — git-edge server

Surface: inputs that hit Cloudflare plan limits (128 MB isolate, subrequests, CPU/DO storage, R2 ops)
or degrade badly. Method: full read of `server/src` (~4.9k lines) with per-charge-call tracing of
`ReqBudget` (lib.rs:33-39), memory accounting in `pack/ingest.rs` + `pack/generate.rs`, and every
SQLite query shape in `store`, `repo_do`, `jobs`. No live probes were run (no exec in this session);
all reproducers are deterministic reasoning or commands. Tree was mid-edit during audit; all cites
re-verified after the change.

## QA Findings

### Critical (blocks release)

None on this surface — nothing found that corrupts data or bypasses auth through scale alone.
The worst issues are availability (DoS/OOM): below.

### High (should fix)

1. **Fetch stream never terminates on error — infinite band-3 `ERR` loop**
   - `repo_do/mod.rs:644-655`: on `Err` from `st.step()`, `stream::unfold` emits one `ERR` frame and
     returns `Some((Ok(w.out), st))` with `st.next` **unchanged** — the next poll retries the exact
     same failing operation. For a deterministic failure (Budget wall-clock expiry mid-stream via
     `charge()` at `lib.rs:35`; `Storage("missing key")` from a pack the janitor deleted under a
     >1 h stream; `Storage("short range read")` at `generate.rs:421`) this is an infinite sequence of
     `ERR` frames, produced synchronously — `pack_chunk` fails at `budget.charge(1)?`
     (`store/mod.rs:267`) *before* any await, so each retry is a tight no-yield loop.
   - Expected: one ERR frame, then `None` (comment at line 649 even says "then the stream ends").
   - Actual: request never ends; a client that keeps draining gets an unbounded stream of ERR frames
     and the DO fetch event burns CPU for as long as the connection lives.
   - Repro: `git clone` a repo whose fetch hits the 240 s `max_ms` mid-stream (any slow/large clone),
     or delete the R2 pack key under an in-flight stream. The stream emits `ERR request budget
     exhausted` frames forever.
   - Fix: on `Err`, emit the frame once and mark `st` terminal (e.g., `st.next = usize::MAX` or a
     `done: bool` flag) so the next poll returns `None`.

2. **Clone depth ceiling ≈ 9,000 commits — deep repos are unclonable**
   - `generate.rs:135-144`: the commit walk loads each BFS level with one `load()` →
     `Bucket::read_entries` → one `read_range` **per coalesced span** (`store/mod.rs:286-326`). For a
     mostly-linear history the queue is ~1 commit per level, so each level costs ~1 R2 subrequest for
     a ~500-byte entry. Reads consumed ≈ *history depth*, not object count. `ReqBudget` is 9,000
     (`lib.rs:27`), shared with the tree/blob walk and `plan_reads`, so the practical ceiling is well
     under 9,000 levels.
   - `mem.clear()` per level (`generate.rs:196`) guarantees no span reuse across levels, and
     CONTRACTS.md §7.4's mitigation — one cached `[commit_lo, commit_hi)` range read per pack — is
     **not implemented**: `load` (generate.rs:294-315) fetches only the requested entries.
   - Expected: a 50k-commit mainline clones. Actual: `Error::Budget` → `ERR` mid-response at level
     ~8.5k. The error text "clone instead" (`generate.rs:23`) is self-referential — clone *is* this
     path.
   - Repro: `git clone` a repo whose first-parent chain exceeds ~9,000 commits (linux.git-scale is
     hopeless; even a tidy 10k-commit project fails).
   - Fix: implement §7.4 — read whole commit regions per pack per request, or batch the frontier
     across levels.

3. **`load()` enforces `MAX_MEM` only after allocating everything — OOM before the cap fires**
   - `generate.rs:306-311`: `read_entries` builds `out` = one `Vec<u8>` per entry for *all* requested
     locs (up to 10,000 per chunk, `CHUNK` at line 19), then `decode_entry` inflates each into `mem`,
     and only then is `mem.bytes > MAX_MEM` checked. A chunk of 10k trees at ≤16 MiB inflated each
     means ~100+ MB of compressed entries + GB-scale decode demand in a single call — the isolate
     dies at 128 MB long before `Limit` can return.
   - Same pattern elsewhere: `seen`/`depth`/`next`/`blobs` grow until `MAX_OBJECTS` is checked
     *after* `seen.insert` (`generate.rs:350-351`); `idx.lookup` materializes all rows.
   - Repro: repo with ~200+ distinct large trees reachable in one chunk window, or ~200k commits in
     one BFS level → any clone → isolate OOM (500 to client, all in-flight DO work killed).
   - Fix: check `mem.bytes` inside the insert loop and cap the *byte* size of each `read_entries`
     call (like `gc.rs`'s `chunks()` LOAD=32 MiB does at gc.rs:136-153 — the send-set path has no
     equivalent).

4. **Negotiated fetch is quadratic in memory and CPU: `items = trees × bases.clone()`**
   - `generate.rs:215-216`: every queued tree owns `bases.clone()` (≤64 ids after
     `bases.truncate(64)` at line 212). ~1.3 KB per item × 200k trees ≈ 260 MB → OOM.
   - `generate.rs:333-341` (`expand_tree`): every tree re-parses *all* of its ≤64 base trees
     (`TreeRefIter` per base) to build `had`/`by_name`. 100k trees × 64 bases × ~1k entries/base ≈
     6×10^9 entry parses → CPU-limit kill inside one request.
   - `generate.rs:221-227`: the same ≤64 base trees are re-read from R2 **per 10k-tree chunk**
     (`load(&bs)` with `mem.clear()` per chunk) — up to ~64 extra subrequests per chunk.
   - Trigger needs only `deepen-not <oid>` or ~64 `have` lines — no large upload required.
   - Repro: on a ≥100k-tree repo, send a `fetch` with `deepen-not` pointing at 64 scattered commits
     (or 64 haves that become edge bases) + `done`. One request → OOM or 300 s CPU burn.
   - Fix: hoist base parsing into a per-chunk `HashSet`/map built once; store `bases` as
     `Rc<Vec<ObjectId>>` or a shared index, not per-item clones.

5. **`SendSet::mark` is O(#packs) per call — O(objects × packs) total**
   - `generate.rs:47`: `self.packs.iter().position(|p| p.pack == loc.pack)` — a linear String-compare
     scan per marked object; `mark` is called once per commit (172), tree (225), blob (236), and tag
     (254). A clone touching P packs and marking M objects does ~M×P/2 comparisons plus a
     `pack_meta` query per new pack (line 50).
   - At 5k packs × 1M objects ≈ 2.5×10^9 compares → fetch dies on the 300 s CPU limit in a pure-CPU
     stretch that no `charge()` call ever interrupts (see #15 on time checks).
   - Fix: `HashMap<PackId, usize>` index into `self.packs`.

6. **Pack ingest cannot reach its advertised extremes — OOMs below 2M entries**
   - `EntryRec` vec: 2M × 24 B = 48 MB (`ingest.rs:27,100`).
   - `by_id`: enabled for *any* pack containing a ref-delta (`ingest.rs:357,401-403`) — 2M × ~50 B ≈
     100-160 MB. A 2M-entry thin pack OOMs on this + recs alone.
   - `sink.links`: `sink.links.extend(links)` per entry (`ingest.rs:390`) accumulates *every*
     extracted link for the whole pass; `MAX_LINKS` is consulted only inside `post_meta`, i.e., once
     per 10,000 rows (`run.rs:138-139`). Between checks, growth is unbounded: a 16 MiB tree has
     ~300k links, and ~35 such trees (a ~600 KB compressed pack!) exceed the isolate before the
     check ever runs. A normal 2M-object push averaging 2 links/object → 4M+ ids ≈ 100-160 MB → OOM.
   - `resolve_at`'s `chain` holds up to `MAX_DEPTH=64` raw entries (`ingest.rs:283-318`); 64 × ~16
     MiB compressed ≈ 1 GB worst case.
   - `decode_mini` (`ingest.rs:212-253`) builds a mini-pack containing a zlib'd copy of the base
     (~16 MiB) + delta body + `out` ≤16 MiB ≈ ~50 MB transient per delta, on top of Cache 48 MiB +
     window 8 MiB + part buffer 8 MiB + recs.
   - Realistic simultaneous footprint for a worst-case-but-legal pack: ~48 (recs) + ~110 (by_id) +
     ~48 (cache) + ~50 (decode_mini transients) + ~8 (window) + ~8 (part) ≈ **270 MB**.
   - Fix: enforce MAX_LINKS per-entry (not per-post), dedupe links incrementally, bound `chain` bytes
     not just depth, and reuse a `mini` buffer.

7. **`plan_reads`/`entries_of` materializes every marked `ObjLoc` — ~90-250 MB per big pack**
   - `store/mod.rs:694-733`: `entries_of` pages through *all* `objects` rows of the pack (5,000-row
     keyset pages — for a 2M-entry pack that's 400 queries and 2M billed rows-read even when the
     bitmap marks 10 entries), building `Vec<ObjLoc>` where each `ObjLoc` carries a 32-char `String`
     pack id (~56 B struct + 32 B heap). Full clone of a 2M-entry pack ≈ 180-250 MB → OOM inside
     `send_set` before `plan_reads` finishes (`generate.rs:366`).
   - Fix: stream `entries_of` into `coalesce` per page; store the pack index (`u16`) in the locs
     instead of `PackId(String)`.

8. **GC starvation under ordinary push cadence → packs accumulate → repo becomes unclonable**
   - Every push writes a pack; every fetch pays ≥1 subrequest per touched pack (read-per-span +
     `plan_reads` floor of ~1 read/pack, `generate.rs:363-380`) plus the `mark` scan (#5) and
     `entries_of` rows (#7).
   - `gc_mark` re-checks `refs_version` every 512-id batch and aborts the whole chain on any change
     (`gc.rs:202-204` → `abort` at 296-311). A repo receiving pushes more often than the mark+
     consolidate+ sweep chain completes never consolidates — packs grow without bound, each fetch
     gets slower, and eventually every clone hits the 9,000-subrequest cap. Self-reinforcing
     availability cliff reachable with no malice at all.
   - Fix: tolerate `refs_version` drift during mark (marks are conservative — reachable set only
     grows) or slice GC so a changed version restarts only the sweep phase.

### Medium (nice to fix)

9. **`gc_mark` bitmap N+1**: one `SELECT bitmap FROM marked` per newly-touched candidate pack per
   batch (`gc.rs:236-243`); `bits` accumulates *all* packs touched by a 512-sha batch before the
   first `commit_ids` drain (gc.rs:288) → up to ~512 × 250 KB bitmaps ≈ 128 MB held at once for
   2M-entry packs. Each `commit_ids` then rewrites every touched bitmap blob — 250 KB BLOB write per
   pack per round → heavy billed rows/bytes on wide-touching batches.

10. **`gc_mark` `kids` unbounded → OOM → permanent GC wedge.** `kids.push` per link with no cap
    (gc.rs:253-258); 64 trees × ~300k entries ≈ 19M hex strings ≈ ~800 MB, plus a single
    `json_each(?)` param of that size at `commit_ids` (gc.rs:283-286). An isolate OOM mid-slice is
    not an `Err` — the job row stays `running`, `repair` requeues it after 60 s
    (`jobs/mod.rs:113-119`), `attempts` is never incremented, and the same batch kills it again
    forever: the 'dead'-after-8-attempts escape (jobs/mod.rs:233) is unreachable for OOM kills.

11. **`gc_sweep` mega-transaction**: `DELETE FROM objects WHERE pack_id IN (SELECT pack_id FROM
    marked)` (gc.rs:641) is one sync span over potentially millions of rows — a candidate for the DO
    event CPU ceiling; the span rolls back on kill and the sweep retries into the same wall
    (correctness-safe, availability-wedged).

12. **Unindexed scans on forever-growing tables.** `pushes` rows are never deleted (janitor sets
    `swept_at` only, janitor.rs:101), so `push_begin`'s `SELECT COUNT(*) WHERE state='open'`
    (repo_do:340) and the janitor's `pushes`/`packs` scans (janitor.rs:35,51,69,88,105) are full
    table scans billed as rows-read on every push and every 15-min slice — O(repo history) per
    operation. Add indexes on `pushes(state)`, `packs(state,dead_at)`, `packs(state,created_at)`.

13. **Pass-B window thrash on far-apart delta bases.** `Window::entry` slides `[offset,+8MiB)`
    (`ingest.rs:180-200`); `resolve_at` walks bases *backward* (ingest.rs:295) — each out-of-window
    base = 1 range read, and the next sequential entry pays another read to slide forward. A pack
    built with `--window=250 --depth=64` (or just very spread deltas) costs ~2 subrequests/entry and
    up to ~65/entry for deep chains → ingest dies at ~9k subrequests on a pack of a few hundred
    thousand entries. Legal git output, fatal here.

14. **`commit_push` is O(commands) inside one sync span.** Header cap 1 MiB ⇒ ~15k `RefCommand`s;
    each costs a name-validate + `idx.lookup` + CAS write + `changes()` + reflog insert
    (repo_do:467-541) ≈ 75k statements in a single span — a candidate for the DO event CPU limit;
    a kill mid-span rolls everything back (safe but the push simply fails).

15. **`include_tag` N+1**: `exec_refs_tags` loads all peeled tag refs (generate.rs:264-272), then one
    `idx.lookup(&[tag])` per matching tag (line 253) — 100k tag refs ⇒ 100k SELECTs per fetch.
    Batch through `idx.lookup` on the collected tag ids.

16. **ls-refs prefix match is O(refs × prefixes).** `wanted` runs `a.prefixes.iter().any(starts_with)`
    per ref (wire/mod.rs:378,400). A 1 MiB body carries ~80k `ref-prefix` args → ~10^10 byte-compares
    against a 100k-ref repo → one cheap request pins the DO. Fix: sort prefixes, or build a prefix
    automaton/trie.

17. **`push_lookup` miss path is O(ids) point queries.** Each id not found in live packs triggers a
    `lookup_in_pack` SELECT (repo_do:367-372) — up to 1,000 per call. A 1M-link push where most links
    are in the same ingesting pack ⇒ ~1M individual SELECTs across the links check
    (`run.rs:106-115`). Batch misses with `json_each`.

### Low

18. **Uncharged subrequests** — small leaks against the "every Bucket call charges first" rule
    (CONTRACTS §7.1): `RawWriter::abort` / `PackWriter::abort` (store/mod.rs:380,501),
    `PackWriter::upload_id` (store/mod.rs:507), and `Bucket::delete` takes no `budget` parameter at
    all (store/mod.rs:328-334) — any future caller is uncounted.

19. **`x-ge-subrequests` response header is never emitted** (grep: zero occurrences in `server/src`)
    though CONTRACTS §7 makes it the harness's only observable for budget correctness — every
    finding above is invisible to `tests/conformance/run.sh`, and local workerd doesn't enforce the
    subrequest ceiling.

20. **Wall-clock is only sampled inside `charge()`** (lib.rs:35): pure-CPU stretches between
    subrequests (the mark scan of #5, giant JSON builds, the commit_push span of #14) run unchecked;
    the effective fuse is the 300 s isolate CPU kill, not the 240 s budget.

21. **Client-forged `shallow`/`deepen` inputs surface as Internal 500s and amplify walks.**
    `deepen 2147483647` puts unvalidated client `shallow` ids into `commits` (generate.rs:126-127)
    → `load` → `Internal("reachable … not live")` → 500, not a protocol error. `deepen-relative`
    walks *through* ack walls (`generate.rs:184-190`) — a cheap fetch can force a near-full history
    walk. Both bounded by MAX_COMMITS but each request is a full-walk DoS unit.

22. **DO route bodies are buffered whole** (`req.bytes()` at repo_do:81) with no size guard —
    `/_do/push/index` posts are ~2.5 MB (fine), but every `/_do/*` route trusts the edge to cap;
    a defense-in-depth body cap would be cheap.

23. **64-open-push ceiling is a billing DoS, bounded but real**: each open push can hold a 2 GiB
    `pending/` pack plus a normalized pack (bounded by ~9k part-uploads ≈ ~70 GB) for up to
    ~PUSH_TIMEOUT+GRACE ≈ 2 h (repo_do:340-343, janitor.rs:33-46,86-101) → one writer can park
    terabytes of R2 garbage per repo before the janitor reclaims it.

24. **Per-request memory caps assume an exclusive isolate.** `MAX_MEM` 64 MiB + `seen` ~60 MB is
    *per request*, but a DO interleaves concurrent `fetch_v2` awaits and an edge isolate can hold
    several concurrent pass-B ingests (~100+ MB each) — two overlapping clones or pushes can exceed
    128 MB with every individual cap respected. There is no global accounting.

### Survived attack (no finding)

- `plan_reads` projects `budget.used + reads.len() ≤ max` *before* streaming (generate.rs:374-377) —
  subrequest overrun can't abort mid-stream on count (only on wall-clock, which feeds #1).
- Every R2 write/read and stub call on the hot paths charges first: `read_range` (store:267),
  `read_entries`→`read_range`, `RawWriter`/`PackWriter` create/parts/complete (store:350,362,372,
  375,421,462,481,485,541), `stub_json`/`stub_raw` (http.rs:110,128), janitor deletes
  (janitor.rs:99,117).
- `stream_to_pending`: entry cap enforced before push (ingest.rs:66), zlib-stall guard (ingest.rs:
  88-90), 2 GiB ceiling via `body.total` (ingest.rs:93) which counts *decompressed* bytes — the gzip
  path can't sneak under it.
- `entries_of` keyset pagination is correct and uses the `objects_pack` index (store:694-733);
  `idx.lookup` batches IN-lists at 90 (store:620-643); the reader query is indexed on `sha`.
- Empty-pack and delete-only push paths are O(1) (run.rs:47-58).
- `SliceBudget` (jobs/mod.rs:57-71) is checked between units of work in every job loop
  (gc.rs:199,261,382; janitor.rs:95,114) — slices do yield.
