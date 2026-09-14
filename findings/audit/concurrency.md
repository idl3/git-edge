# Concurrency audit — races & interleavings

Surface: overlapping pushes, push vs GC, fetch vs push/GC, jobs (dispatch/repair/enqueue/rearm),
and DO event ordering. No exec access in this environment → reproducers are documented
sequences/reasoning. Everything below cites code actually read.

Model used: each DO event = `req.bytes().await` (repo_do/mod.rs:81) → one sync span (boot +
route, :85-99) → optional awaits (`rearm` :112, `fetch_v2` :104, `dispatch` :125). Writes commit
at the first await / handler return; a thrown `worker::Error` rolls back the uncommitted span.
Two sync spans never interleave; two events can interleave only across awaits.

## Critical (blocks release)

None found. The `gc_epoch`/`refs_version` dance between `commit_push` and `gc_sweep` was attacked
in every order and holds (see "verified safe").

## High

### H1. `send_set` is torn by `gc_sweep`/`abort` → fetch emits a malformed PACK (header count > entries delivered)

- Files: `server/src/pack/generate.rs` (`set.mark` at :96, :226, :290, :301, :319; `plan_reads`
  :325/:428-445; `load` :366-370), `server/src/store/mod.rs` (`entries_of` :694-733 — **no
  `p.state='live'` filter**; `pack_meta` :681-687), `server/src/jobs/gc.rs` (sweep span
  :636-646; `abort` :296-311), `server/src/repo_do/mod.rs` (header count from `set.count()` :693).
- Invariant: the PACK the server streams is internally consistent — header object count equals
  entries emitted (pack format; `count()` is the in-memory bitmap popcount, generate.rs:64-66).
- Attack: `send_set` marks bits per `(pack, idx)` across *many* awaits (`load()` awaits
  `read_entries` at :371 between each traversal round). `SendSet` stores only bitmaps; the
  `(offset,len)` for each bit is re-queried at the end by `entries_of` inside `plan_reads`
  (:431). `gc_sweep` (one sync span, gc.rs:636-646) `DELETE`s **all** `objects` rows of every
  `marked` candidate pack and flips them `dead`. If a sweep span lands anywhere between the
  first `set.mark` and `plan_reads`, `entries_of` returns fewer (or zero) locs while the
  in-memory bitmap still has the bits:
    - `set.count()` writes N into the PACK header (:693),
    - `pack_chunk` emits only the M < N entries that `reads` cover,
    - git's index-pack dies (`pack is corrupted` / early EOF) — the client cannot even retry the
      same objects because the response was wire-corrupt, not `ERR`-signalled.
  Same tear via `abort()` (gc.rs:297-303), which deletes `objects` rows of `gc.new_pack` — a
  fetch can have marked bits in the consolidated pack between `finish_build`'s live-flip
  (gc.rs:606-624) and a later refs_version-mismatch abort.
- Expected vs actual: fetch should resolve against one index snapshot or fail cleanly before
  response bytes; instead the index is re-read at plan time against a different generation than
  the marks were taken from.
- Adjacent failures in the same window (all transient 500s, retry heals — still wrong):
    - `load()` post-sweep lookup miss → `Error::Internal("reachable {id} is not live")`
      (generate.rs:369) → RustError → 500 (repo_do/mod.rs:569).
    - `set.mark` post-await → `pack_meta` requires `state='live'` (store/mod.rs:681) →
      `Storage("pack … not live")` → 500.
    - blob loop `reachable blob {b} is not live` (generate.rs:299) → 500.
- Window: only *pre-*`plan_reads`. Once `set.reads` is populated, `pack_chunk` (generate.rs:471-491)
  uses in-memory `(off,len)` + R2, and R2 bytes survive `dead` for GRACE=1h ≫ 240 s request cap
  — mid-*stream* sweeps are safe.
- Likelihood: `GE_GC_QUIET_MS`/`GE_GC_GRACE_MS` env overrides (repo_do/mod.rs:183-190) exist
  precisely to make the GC chain fast; under dev/test settings every multi-round fetch during a
  sweep hits this. In prod it needs a fetch whose send_set straddles the sweep — large repos
  (many traversal rounds = many awaits) widen the window to seconds.
- Reproducer: with `GE_GC_QUIET_MS=2000`, `GE_GC_GRACE_MS=0`: seed repo (≥ a few thousand objects
  so send_set has several rounds), `git push -f` an orphaning tip to enqueue GC, then immediately
  `git clone --filter=blob:none`/`git fetch` in a loop while polling `/_state` until `marked`>0 →
  `packs_dead`>0. Fetches interleaving the `packs_dead` transition fail or hang. Deterministic
  version: two-shot DO drive — post `/_do/fetch` with a want set whose walk is multi-round, fire
  the alarm (local scheduler) mid-send_set.
- Fix: snapshot `gc_epoch` at send_set start and re-check it in `plan_reads` (fail/retry), or
  have `entries_of` `JOIN packs … AND state='live'` **and** assert `locs.len() == bitmap
  popcount` → hard error instead of a corrupt pack. Recording `(offset,len)` in `SendSet` at mark
  time removes the re-query entirely.

### H2. `FetchStream` error arm retries the same step forever — infinite `ERR` frame stream

- File: `server/src/repo_do/mod.rs:657-667`.
- Invariant: section 10 — after response bytes start, "one band-3 ERR frame, **then the stream
  ends**" (comment at :662).
- Actual: `Err(e) => Some((Ok(w.out), st))` returns `st` **unchanged** — `st.next` was not
  advanced because `step()` failed before `self.next += 1` (:709). `stream::unfold` therefore
  re-polls `step()` at the same index; a persistent error produces an unbounded sequence of
  identical `ERR internal error` frames. The stream never returns `None`.
- Persistent-error sources in `pack_chunk` (generate.rs:471-491): repeated `Error::Storage`
  (`missing {key}`, `short range read`, R2 5xx during an outage), `Error::Internal` offset math.
  Each retry also burns `budget.charge(1)` (store/mod.rs:267): after ~9,000 retries `charge`
  returns `Error::Budget` — *also* caught by the same arm — and the loop then spins at CPU speed
  with no I/O until the 300 s CPU limit kills the isolate.
- Impact: one unlucky fetch pins its DO event for up to the CPU limit and the client hangs
  reading `ERR` frames until disconnect (git reads to EOF; it never comes).
- Reproducer: fault-inject `read_range` (or delete an R2 key mid-stream on local workerd) →
  response is an endless run of band-3 `ERR internal error` frames. Reasoned, no exec here.
- Fix: after emitting the ERR frame return a terminal state — e.g. set `st.next = usize::MAX`
  (or a `done` flag) inside the error arm so the next `step()` returns `Ok(None)`.

## Medium

### M1. `repair()` requeues a legitimately-running slice → two interleaved executions of one job

- Files: `server/src/jobs/mod.rs` (`repair` :113-119 — `UPDATE jobs SET state='queued',
  run_at=? WHERE state='running' AND started_at < now-60s`; `started_at` set at :188; outcome
  apply has **no state guard**, `WHERE id=?` only, :212-243), `server/src/repo_do/mod.rs`
  (`repair` runs inside `boot` at :208 → on **every** fetch event, not just alarms).
- The 60 s straggler threshold assumes a slice can't exceed ~20 s (SliceBudget, jobs/mod.rs:56-72).
  But the budget is *checked between units*, and `build()`'s inner chunk loop
  (`gc.rs:433-457`) has **no `spent_80pct()` call** — one outer iteration can await up to
  ~90 sequential `read_entries` (BATCH_N=90, gc.rs:25). Wall clock also inflates when the DO
  shares its thread with concurrent fetches/pushes. A slice suspended at an await past 60 s of
  `started_at` age is requeued by the next request's `boot`; the next alarm re-dispatches the
  same job id while the first slice is still mid-flight → two interleaved executions.
- Impact by kind:
    - GcConsolidate (worst): both slices share `gc.pos`/`gc.new_pack`/the MPU. Two
      `PackWriter::create`s on the same key (gc.rs:337-351) create *different* MPUs; part-number
      collisions, `complete()` etag mismatches → `mpu_err` → Retry/Rebuild churn, wasted
      subrequests, possible `dead` job (which `repair` then re-enqueues → repeat). Build output
      is deterministic ((pack_id, idx) order over immutable marks), so silent corruption is
      unlikely — but the wedge/churn is real.
    - GcMark: `commit_ids` is `INSERT OR IGNORE` — safe — but one copy's `abort` (gc.rs:202-204)
      wipes `gc_*` tables under the sibling; the sibling's next `meta_i64("gc.refs_version")`
      errors → Err → backoff. Converges, wastes work.
    - Janitor/GcSweep: idempotent.
    - All kinds: first finisher's `Done` `DELETE`s the job row (jobs/mod.rs:212) out from under
      the still-running second copy; that copy's outcome-apply then silently no-ops while its
      side effects already committed.
- Reproducer: on local workerd, throttle R2 (or run a large consolidate while spamming fetches);
  poll `/_state` — a `jobs_running` row reappears as `queued` mid-slice, then a second
  `running` mark on the same id. Or directly: set a running row's `started_at` back 61 s via a
  dev hook and issue any request.
- Fix: heartbeats — have `run_slice`/checkpoint spans touch `started_at` (or a `lease_until`)
  so a *progressing* slice is never "stale"; and/or make the outcome-apply `WHERE id=? AND
  state='running' AND started_at=?` so a re-dispatched row can't be clobbered.

### M2. `rearm` is skipped on the `/_do/fetch` route and on every error-response arm → enqueued jobs can be left unarmed forever

- File: `server/src/repo_do/mod.rs:104` (early `return self.fetch_v2(&body).await` bypasses the
  `jobs::rearm(self).await` at :112) and :119 (`Err(e) => Ok(do_error_response(&e)?)` — no rearm).
- `boot` can write job rows in the same span: first-boot `enqueue(Janitor)` (:250) and
  `repair`'s re-enqueues (jobs/mod.rs:134, :149). For `/_do/fetch` — which runs `boot` first
  (:85) — and for any route that ends in `Protocol`/`Conflict`/`Unpack`/`Budget`/`Limit`
  (i.e. everything except `Storage`/`Internal`, whose rollback correctly discards the enqueue),
  the row commits but no alarm is armed.
- Scenario A (deterministic): fresh repo whose **first** DO request is `POST
  /o/r/git-upload-pack` with `command=fetch` (or a malformed v2 command → `/_do/ls-refs` →
  `Error::Protocol` at repo_do/mod.rs:328). `boot` enqueues the Janitor; the route skips rearm;
  no alarm exists → janitor never runs until a *successful non-fetch* event arrives. A
  fetch-only repo (read mirrors, CI clones) never runs the janitor: expired pushes, dead packs,
  `pending/` keys and reflog accumulate forever.
- Scenario B: a repo with `dead` job rows (post-8-failures) receiving only erroring/fetch
  traffic → `repair` re-enqueues every boot, nothing is ever armed → the GC chain is
  permanently stalled.
- Reproducer: `curl -X POST $U/$REPO/git-upload-pack -H 'Git-Protocol: version=2'
  --data-binary '<command=fetch … want zeros>'` on a brand-new repo, then `GET /$REPO/_state`
  → `jobs_queued: 1` forever; no janitor fires.
- Fix: run `rearm` unconditionally after `boot` (it is cheap and idempotent), or at minimum on
  the fetch path and the `do_error_response` arm — the enqueue sites are all inside `boot`'s
  span.

### M3. Janitor's orphan rule kills an in-progress GC build pack → consolidate wedges permanently

- Files: `server/src/jobs/janitor.rs:67-82` (orphan rule), `server/src/jobs/gc.rs:580-585`
  (build pack inserted `state='ingesting', push_id=NULL, created_at=begin_time`),
  `gc.rs:619-621` (`finish_build` fails on `changes()!=1`), `gc.rs:606-624`, `jobs/mod.rs:230-243`.
- The guard `NOT EXISTS(SELECT 1 FROM pushes u WHERE u.pack_id = packs.id AND u.state='open')`
  is **vacuous for every ingesting pack**: `pushes.pack_id` is `NULL` until `finish_push`
  (repo_do/mod.rs:553). For normal pushes this is masked by `created_at ≥ began_at` (the push
  expires first at janitor.rs:33-46). For the GC build pack (`push_id=NULL`) the *only* guard is
  `created_at < now - 1h`.
- Attack/sequence: a consolidate whose `created_at` ages past `PUSH_TIMEOUT` (1 h) mid-build —
  reachable two ways: (a) a genuinely large repo where slice+retry wall time exceeds 1 h;
  (b) retry backoff: `run_at = now + min(30 s·2^attempts, 1 h)` (jobs/mod.rs:230-232) — a
  couple of `mpu_err` retries push the next slice past the pack's first hour. Then: janitor
  deletes its `objects` rows and marks it `dead` → the resumed build re-inserts rows (FK not
  enforced) → `out.finish()` → `finish_build`'s `UPDATE … WHERE state IN ('ingesting','live')`
  matches 0 → `Err(Internal("build pack row"))` → job backoff → same failure → `dead` at 8
  attempts → `repair` re-enqueues `GcConsolidate` with `gc.pos` still pointing at the dead pack
  → loop forever. `wipe_build`/`Rebuild` (gc.rs:592-604) is only reached via `mpu_err` (R2
  failure), never via this SQL error — so the chain never recovers: every later `GcMark`
  reschedules on the `busy` check (gc.rs:161-169) because a consolidate row is always
  queued/running/dead-cycling. GC is dead-locked for the repo.
- Fix: exclude `push_id IS NULL` (or `gc.pos`-tracked) packs from the orphan rule, or stamp a
  heartbeat on the build pack / refresh `created_at` at each checkpoint commit.

## Low

- **L1. `commit_push` does not bind `pack_id` to the push.** `UPDATE packs SET state='live'
  WHERE id=? AND state='ingesting'` (repo_do/mod.rs:454-460) lacks the `AND push_id=?` that
  `push_index`'s upsert has (:398-402). An edge-side mixup (retry/shuffle handing push A the
  pack id of a still-ingesting push B) flips B's half-indexed pack live → readers can resolve
  objects whose entries were never uploaded → short reads → corrupt packs; B's own commit then
  fails `pack not in state ingesting`. Not client-reachable (ids are server-generated and never
  leave the edge), hence LOW — but it's a one-clause fix for a one-way door.
- **L2. `dispatch` *can* let an error escape — contract 4.4 violated.** `dispatch` returns `r`
  (jobs/mod.rs:162); `alarm()` propagates it with `?` (repo_do/mod.rs:125). Any `d.q`/`exec`
  failure in dispatch bookkeeping (:176-243) or `JobKind::of` (:196) escapes → the platform's
  own alarm retry fires → a persistent SQLite fault produces an alarm retry storm (each retry
  re-runs `boot`+`repair`+`dispatch`). The comment at :155 claims the opposite.
- **L3. `rearm` last-writer-wins ordering.** Two concurrent events both ending in `rearm`
  (jobs/mod.rs:89-109) race their `set_alarm` host calls; the later-completing wins. A stale,
  later `run_at` can overwrite an earlier one → queued jobs fire late (bounded by the stale min,
  never lost, because a queued Janitor row virtually always exists — making `delete_alarm` at
  :106 nearly unreachable). No starvation proven; worth a comment, not a fix.
- **L4. R2-delete failures are swallowed and the row is then deleted → orphaned keys.**
  `let _ = d.bucket()?.inner.delete(key).await` (janitor.rs:100, :118) is followed by
  `UPDATE pushes SET swept_at`/`DELETE FROM packs` (:101, :119). If the delete fails, the row
  is gone and the key is never retried → permanent R2 leak. Same class: pass-A `RawWriter`'s
  MPU is dropped un-aborted when `stream_to_pending` errors (pack/ingest.rs:49 + run.rs:60 `?`),
  and `PackWriter` MPUs abandoned between `post_meta(EMPTY)` (run.rs:81) and `create` (run.rs:83)
  orphan similarly — R2's ~7-day MPU expiry covers it, but it's uncounted.
- **L5. `want_refs`/`deepen_not`/`include_tag` read `refs` mid-event.** repo_do/mod.rs:587-622
  and generate.rs:309 (`exec_refs_tags` — after awaits) read the refs table at a different
  generation than the client's ls-refs advertisement → a deleted ref yields `couldn't find
  remote ref` (correct per protocol) and a tag created mid-fetch may or may not ride along —
  both benign, noted for completeness.

## Verified safe (no finding)

- **Two pushes, same ref.** Both commits are single sync spans (repo_do/mod.rs:421-478); the
  `UPDATE refs … WHERE target=?` + `changes()` CAS (:522-528) serializes them — the loser gets
  `ng failed to update ref`. Two commits cannot both pass one CAS. A replayed/second commit on
  the same push id hits `push.state != 'open'` → `Conflict` (:441-443).
- **Pack live-flip on an expired push — impossible.** `commit_push` checks `state='open'` in
  the same span as the flip (:441, :454-458); `push_index` refuses non-open pushes before
  touching rows (:389-396). Janitor's expire is `WHERE state='open'` CAS'd (janitor.rs:42-45)
  — whichever event runs first wins cleanly.
- **Push vs sweep — the epoch guard holds in every order.** Sweep-then-commit → `gc_epoch`
  mismatch → all refs `ng gc ran during push, retry` (:444-450); commit-then-sweep →
  `refs_version` mismatch → `abort` (gc.rs:633-635); sweep-between-lookup-and-commit → same
  epoch rejection. Abort-killing `gc.new_pack` under a push that resolved bases there leaves
  no hole: the candidates stay `live` on abort and the push's own pack is self-contained
  (ingest resolved bases into it).
- **Fetch mid-*stream* vs sweep** — safe: `pack_chunk` reads R2 by `(off,len)` captured at plan
  time; deleted packs keep their R2 bytes for GRACE=1 h ≫ 240 s request budget. The vulnerable
  window is only pre-`plan_reads` (→ H1).
- **Fetch vs push** — no tear that matters: objects are immutable and content-addressed; a
  want that resolves pre-commit vs post-commit yields the same bytes; a want on a not-yet-live
  oid is a clean `not our ref` ERR (generate.rs:95).
- **`enqueue` dedup** — suppressing a redundant run is always safe: every enqueued kind re-reads
  its snapshot (`gc.refs_version` captured at the mark's first slice, gc.rs:176); a stale queued
  row can't encode a stale decision. The running-row double-enqueue is intentional (jobs/mod.rs:74-77).
- **`marked`/`gc_frontier` replay safety** — `commit_ids` (gc.rs:274-292) is `INSERT OR IGNORE`
  + delete-popped; a kill mid-round replays idempotently, as designed.
- **Consolidate live-flip mid-fetch** — the new pack going `live` between a fetch's awaits only
  means `idx.lookup` may return its rows; duplicate live rows for one sha are byte-identical
  (contract 2.3).
- **Body-read await (:81) before every span** — reordering only picks which event's span runs
  first; no state is mutated before it.

---

## Summary of the two findings I'd act on before release

1. **H1** is the flagship race: `send_set` takes marks from the index at time T but re-reads `objects` rows at plan time T′ — `gc_sweep` and `abort` both delete rows in between, and nothing on the fetch path checks a generation counter. The result is a **byte-corrupt PACK on the wire** (or a 500), self-healing only on retry.
2. **H2** is a one-line-shaped DoS: the stream error arm returns the unchanged state, so any persistent `pack_chunk` failure produces an unbounded `ERR` frame stream — directly contradicting the section-10 comment that says the stream ends.

The job-queue findings (M1–M3) all trace to the same root: `repair`'s 60 s stale check and the janitor's 1 h orphan cutoff both assume work units are short, but `gc_consolidate`'s build loop can legally exceed both.
