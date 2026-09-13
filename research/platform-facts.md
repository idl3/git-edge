# Platform facts, measured

Date: 2026-09-13. Runtime: workerd via wrangler 4.129.0, `compatibility_date` 2025-09-01, `nodejs_compat`, run locally. Local workerd is the same runtime Cloudflare runs in production, so the Durable Object, SQLite, alarm, and zlib results below are authoritative. R2 is simulated locally, so the multipart and subrequest results are marked as such. Spike source: `spikes/platform-facts/`.

| # | Question the reviews left open | Answer | Effect on the plan |
|---|---|---|---|
| 1 | Does `cursor.rowsWritten` count index rows on a table with a primary key? | **Yes.** `INSERT` into `refs(name TEXT PRIMARY KEY, ...)` reports `rowsWritten = 2`. `UPDATE` and `DELETE` that match one row report 1. A no-match `UPDATE` reports 0. On a `WITHOUT ROWID` table the `INSERT` reports 1. `SELECT changes()` reports 1 / 0 / 1 correctly in every case, including inside `transactionSync`. | The reviewer was right. Never use `rowsWritten` as the compare-and-swap outcome. Use `SELECT changes()` after the statement, inside the same synchronous transaction. |
| 2 | Is `ctx.id.name` populated inside a DO created with `idFromName`? | **Yes, locally.** `ctx.id.name` returned `"owner/repo"`. | Five reviews claimed it is undefined. On workerd 4.129 it is populated. Keep the rule "store owner/repo in the `meta` table on first request" as belt and braces, but defect 9 is downgraded from "breaks" to "verify in production once". |
| 3 | Can a Worker inflate one zlib object at a time and learn how many input bytes it consumed? | **Yes.** `node:zlib` is available under `nodejs_compat`. `inflateSync(buf, { info: true })` returns `{ buffer, engine }` and `engine.bytesWritten` is the number of input bytes consumed. Two back-to-back zlib streams were split correctly at 24 and 23 bytes. Streaming `createInflate()` also exposes `bytesWritten`. `DecompressionStream` exists but cannot report the boundary. | The streaming pack parser can be written in TypeScript with `node:zlib`. In Rust, `gix-zlib` does the same. This closes the second of the two "verify by hand" items. |
| 4 | Does an `await` on R2 inside a Durable Object let other requests interleave? | **Yes, exactly as the reviews said.** Eight concurrent calls that read a counter, awaited an R2 put, then wrote the counter ended with the counter at 1: seven lost updates. The same pattern awaiting DO storage instead of R2 ended at 8: zero lost updates. A synchronous SQL compare-and-swap placed after the R2 await had exactly one winner out of eight. | Defect 5 is confirmed and its fix is verified. The CAS must be a synchronous `sql.exec` with `changes()` checked, after all network awaits. |
| 5 | Does a second `setAlarm` cancel the first? | **Yes.** Alarm A at +500 ms and alarm B at +1500 ms were set in that order. Only one alarm fired, at B's time. | Defect 6 is confirmed. The single-alarm dispatcher with a `jobs` table is required. |
| 6 | Can a Durable Object drive an R2 multipart upload with 5 MiB parts and then range-read the result? | **API shape works** on the local R2 simulator: two parts of 5 MiB and 1 MiB completed into a 6 MiB object with a two-part etag, and a 16-byte range read at an offset returned 16 bytes. | Not yet verified against real R2. The 5 MiB minimum part size and the part-size equality rule are enforced by real R2 and not by the simulator. Re-run after deploy. |
| 7 | Where does the subrequest limit bite from inside a DO? | **Not enforced locally.** 1,200 R2 `head` calls succeeded. | Must be measured on a deployed Worker. Cloudflare's limits page says 50 on the free plan and 10,000 on the paid plan. |

## What this changes in the documents

- `CONTRACTS.md`: the ref transaction must use `changes()`, never `rowsWritten`. The DO name may be read from `ctx.id.name` but is also stored on first request.
- `findings/cross-cutting-defects.md`: defects 5 and 6 move from "reviewer's claim" to "measured". Defect 9 gets a correction note.
- `findings/build-order.md`: the two "check by hand first" items are both answered. Item 1: yes, index rows are counted, use `changes()`. Item 2: yes, per-object inflate with consumed-byte reporting works.

## Still open, needs a deployed Worker

1. The subrequest limit from inside a Durable Object on the paid plan.
2. Real R2 multipart with parts of at least 5 MiB and a range read on a multi-GB object.
3. Cold start time of a Rust Worker that carries the gitoxide crates.
4. Whether the 128 MB isolate limit is hit by a 50 MB blob inflate in one request.

Deploying needs a Cloudflare API token and account id in this session's environment as `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`. The MCP connector in this session can list and create R2 buckets but cannot deploy a Worker.
