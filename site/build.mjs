import fs from "node:fs";
const rows = JSON.parse(fs.readFileSync("data-full.json", "utf8"));
const th = JSON.parse(fs.readFileSync("themes.json", "utf8"));
const meta = {
  "input-gates": { why: "Durable Objects are single-threaded, but the input gate only holds across storage awaits. Any await on R2 or fetch lets another request interleave, so a ref read before an R2 call and a ref write after it is a classic lost update.", fix: "Do all R2 work first, then run the ref compare-and-swap inside one synchronous transactionSync with no await in it. Reserve blockConcurrencyWhile for GC-style sweeps." },
  "one-alarm": { why: "A Durable Object has exactly one alarm. The janitor, the repack, the CI chain, the KV publisher and the lease expirer all called setAlarm and silently cancelled each other.", fix: "One alarm dispatcher per DO: a jobs table in SQLite with next_run_at, and a single alarm() that pops the earliest row and re-arms for the next." },
  "subrequests": { why: "R2 binding calls count as subrequests, capped at 1,000 per invocation. A blobless checkout, a 1,000-file workspace push, or a pack rebuild all issue one R2 call per object.", fix: "Batch objects into packs and read them with coalesced range reads. Spread multi-thousand-object jobs across alarm slices with a durable cursor." },
  "wire-framing": { why: "Git checks exact bytes: pkt-line lengths include their own four hex digits, sideband frames must be at most 65,515 data bytes, report-status rides in band 1, and a v2 fetch with done omits the acknowledgments section.", fix: "One shared pkt-line and sideband codec module, tested against a stock git client before any feature uses it." },
  "storage-contract": { why: "The proofs disagreed on whether R2 holds zlib loose objects, raw content with metadata, or packs, and on key layout. Ideas that were individually fine could not read each other's bytes.", fix: "Write one object storage spec: key layout, body encoding, metadata fields, and a pack index table shape. Every idea reads through the same object reader." },
  "packs-and-deltas": { why: "Real pushes arrive as thin packs with ofs-delta and ref-delta entries. Many proofs assumed loose objects and had no delta resolution path.", fix: "The streaming pack parser resolves deltas on ingest, and a pack index in SQLite lets every later reader find any object in a pack by range read." },
  "gc-race": { why: "A janitor or repack computed an orphan set, awaited R2, and deleted an object that a concurrent push had just made reachable.", fix: "Grace period by timestamp, refs_version check atomic with the delete, and never delete in the same alarm slice that computed the candidate set." },
  "id-name": { why: "Inside a DO created via idFromName, ctx.id.name is undefined. Proofs that built R2 prefixes from it wrote to objects/undefined.", fix: "Persist owner and repo into SQLite on the first request and read them from there." },
  "request-bodies": { why: "git gzips small POST bodies and sends chunked transfer for pushes over 1 MiB, so there is no content-length to hand R2.", fix: "Honor Content-Encoding at the edge and ingest packs through R2 multipart upload with buffered parts of at least 5 MiB." },
};
const themes = th.map(t => ({ ...t, ...meta[t.key] })).sort((a, b) => b.slugs.length - a.slugs.length);
const waves = [
  { title: "Handshake and spine", note: "The smart-HTTP entrypoint, one DO per repo with ref CAS, refs in SQLite and objects in R2, the streaming pack parser, two-phase push and negotiation. Nothing else exists until git clone and git push work against this.", slugs: ["info-refs-endpoint", "auth-and-multitenancy", "repo-do-ref-authority", "refs-sqlite-objects-r2", "content-addressed-r2-keys", "streaming-pack-parser", "two-phase-push", "want-have-negotiation", "protocol-v2-only"] },
  { title: "Clone at scale", note: "Packs, GC and the fast clone paths. This is where the pack index and the alarm dispatcher earn their keep.", slugs: ["gc-and-repack-alarm", "precomputed-clone-pack", "bundle-uri", "pinned-delta-bases", "partial-clone-filters", "native-lfs", "replicated-refs-edge", "in-do-object-cache"] },
  { title: "Cheap wins on the DO", note: "Everything here is a few extra SQLite rows in the same transaction as the ref CAS, or an outbox drained by an alarm. Days each, not weeks.", slugs: ["ref-leases", "time-travel-refs", "signed-reflog", "r2-versioned-snapshots", "scoped-token-remotes", "commit-event-stream", "github-webhook-compat", "hooks-as-workers", "alarm-chain-ci", "live-fetch-websocket", "search-index-on-push", "ephemeral-repos", "tui-rpc-push", "storage-tiering"] },
  { title: "Needs the Wasm core", note: "Merge, rebase, diff and blame all need real git object algebra. Land gitoxide in Wasm once, behind a host-does-IO boundary, and these unlock together.", slugs: ["wasm-git-core", "server-side-merge", "server-side-rebase", "diff-api-range-reads", "semantic-diffs", "agent-blame", "zero-clone-vfs", "offline-browser-client", "merkle-proofs"] },
  { title: "Exotic, still reachable", note: "Reviewed as risky or caveated, but each has a plausible route once the waves above are solid.", slugs: ["agent-native-commands", "vectorized-commit-graph", "reviews-as-refs", "global-dedup", "cow-forks", "branch-level-dos", "presigned-direct-upload", "speculative-packs", "server-side-bisect", "branch-preview-workers", "time-boxed-history", "cross-repo-atomic-push", "federated-gossip", "client-key-encryption", "git-as-db-driver"] },
  { title: "Does not land as stated", note: "Stock git rejects divergent pushes client-side, so a live CRDT branch cannot be driven by plain git. The closest thing that works is a CRDT document materialized into a normal branch by the DO.", slugs: ["crdt-branches"] },
];
const all = new Set(waves.flatMap(w => w.slugs));
const missing = rows.filter(r => !all.has(r.slug)).map(r => r.slug);
if (missing.length) throw new Error("unwaved: " + missing);
const slim = rows.map(r => ({
  id: r.id, tier: r.tier, slug: r.slug, title: r.title, desc: r.desc,
  proof: { mechanism: r.proof.mechanism, primitives: r.proof.primitives, dependsOn: r.proof.dependsOn.filter(d => rows.some(x => x.slug === d)), keySnippet: r.proof.keySnippet, knownLimits: r.proof.knownLimits },
  review: { feasibility: r.review.feasibility, reliability: r.review.reliability, correctness: r.review.correctness, verdict: r.review.verdict, blockers: r.review.blockers, caveats: r.review.caveats, effort: r.review.effort, summary: r.review.summary },
  proofCode: r.proofCode, why: r.why, reviewMd: r.reviewMd,
}));
const safe = s => s.replace(/<\/script/gi, "<\\/script");
let html = fs.readFileSync("template.html", "utf8");
html = html.replace("__DATA__", () => safe(JSON.stringify(slim))).replace("__THEMES__", () => safe(JSON.stringify(themes))).replace("__WAVES__", () => safe(JSON.stringify(waves)));
fs.writeFileSync("git-edge.html", html);
console.log("bytes", html.length);
