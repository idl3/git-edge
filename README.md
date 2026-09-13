# git-edge

A git smart-HTTP server with no server. Refs live in one Cloudflare Durable Object per repository, objects live in R2, and every push is a compare-and-swap on a SQLite row. Stock `git` clients talk to it over protocol v2 and never know the difference.

This repository holds the feasibility study that came before any code: 56 ideas, each given minimal proof code on real Workers, Durable Object and R2 API shapes, then handed to a skeptical reviewer who tried to break it. Nothing here has run against a Cloudflare account yet. Treat the proofs as an argument, and the reviews as the counter-argument.

## The verdict

| Verdict | Count |
|---|---|
| lands | 1 |
| lands with caveats | 33 |
| risky | 21 |
| does not land | 1 |

Reviewers were told to default to skepticism, so "lands with caveats" is the real pass grade. The one clean pass is the smart-HTTP handshake and pkt-line codec. The one failure is multi-writer CRDT branches: stock git refuses divergent pushes client-side before any bytes reach a server.

The core thesis holds. One Durable Object per repo doing a synchronous SQLite compare-and-swap on refs reproduces exactly what `git-receive-pack` does. Content-addressed R2 keys make retries idempotent. No reviewer found a split-brain path in that spine.

## Start here

- [findings/scoreboard.md](findings/scoreboard.md): every idea scored, with the reviewer's two-sentence summary.
- [findings/cross-cutting-defects.md](findings/cross-cutting-defects.md): the nine bugs that kept reappearing, and the one fix for each.
- [findings/build-order.md](findings/build-order.md): six waves, from the handshake to the exotic.
- [findings/dependency-graph.md](findings/dependency-graph.md): what leans on what.
- [site/index.html](site/index.html): the interactive map with every proof and review one click away.

## Layout

| Folder | What is in it |
|---|---|
| `ideas/` | The catalog of 56 ideas as they went into the workflow |
| `proofs/` | One file per idea: mechanism, primitives, proof code, why it works, known limits |
| `reviews/` | One file per idea: scores, crash walk-through, concurrency walk-through, interop check, blockers, verdict |
| `findings/` | The synthesis: scoreboard, cross-cutting defects, build order, dependency graph |
| `site/` | The interactive map as a single static page, plus the template and build script |
| `data/` | The structured results every other file was generated from |

## The cross-cutting defects in one breath

- **pkt-line and sideband framing details git actually checks** (37 proofs).
- **Janitor or GC sweeps race live pushes and delete referenced objects** (31 proofs).
- **Real pushes arrive as thin packs; every reader needs delta resolution** (27 proofs).
- **DO input gates open during R2 awaits, so check-then-act races** (15 proofs).
- **Proofs disagree on R2 key layout and object encoding** (15 proofs).
- **A Durable Object has exactly one alarm slot; siblings clobber each other** (10 proofs).
- **git sends chunked and gzipped request bodies** (8 proofs).
- **1,000 subrequests per invocation; per-object R2 calls blow the cap** (7 proofs).
- **ctx.id.name is undefined inside a DO made via idFromName** (5 proofs).

## Build order in one breath

0. **Handshake and spine**: 9 ideas.
1. **Clone at scale**: 8 ideas.
2. **Cheap wins on the DO**: 14 ideas.
3. **Needs the Wasm core**: 9 ideas.
4. **Exotic, still reachable**: 15 ideas.
5. **Does not land as stated**: 1 ideas.

## Two things to verify by hand first

1. Whether `cursor.rowsWritten` on Durable Object SQLite counts index-row writes on a table with a primary key. Several proofs use it as the compare-and-swap outcome.
2. Whether workerd exposes `node:zlib` with consumed-byte reporting for per-object inflate inside a packfile. `DecompressionStream` cannot stop at object boundaries.

## How this was produced

Each idea was handed to one agent with a fixed brief: prove the mechanism in 40 to 120 lines on real Cloudflare API shapes, name the primitives, name what you had to hand-wave. Each proof was then handed to a second agent with a different brief: default to skepticism, walk one crash and one concurrent-writer scenario, name the exact wire detail that would break a real git client. The synthesis in `findings/` and `site/` was assembled from those 112 outputs.
