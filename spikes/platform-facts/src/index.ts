// Platform-facts spike for git-edge. Each route answers one open question
// about workerd / Durable Objects / R2 that the feasibility reviews left open.
import { DurableObject } from "cloudflare:workers";

export interface Env {
  FACTS: DurableObjectNamespace<FactsDO>;
  BUCKET: R2Bucket;
}

const json = (o: unknown, status = 200) =>
  new Response(JSON.stringify(o, null, 2), { status, headers: { "content-type": "application/json" } });

export class FactsDO extends DurableObject<Env> {
  counterNet = 0;
  counterStorage = 0;
  log: string[] = [];

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS refs (name TEXT PRIMARY KEY, sha TEXT NOT NULL, updated_at INTEGER)`);
    ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS norowid (name TEXT PRIMARY KEY, sha TEXT NOT NULL) WITHOUT ROWID`);
    ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v INTEGER)`);
  }

  // Q1: what does cursor.rowsWritten report for INSERT / UPDATE / DELETE on a PK table,
  // and what does changes() report? Reviews claimed rowsWritten counts index rows too.
  async rowsWritten() {
    const sql = this.ctx.storage.sql;
    sql.exec(`DELETE FROM refs`); sql.exec(`DELETE FROM norowid`);
    const out: Record<string, unknown> = {};
    const rec = (label: string, c: SqlStorageCursor<any>) => {
      // consume cursor so rowsWritten is final
      const rows = c.toArray();
      out[label] = { rowsRead: c.rowsRead, rowsWritten: c.rowsWritten, rows };
    };
    rec("insert_new", sql.exec(`INSERT OR IGNORE INTO refs (name, sha, updated_at) VALUES ('refs/heads/main','aaaa',1)`));
    rec("insert_dup_ignored", sql.exec(`INSERT OR IGNORE INTO refs (name, sha, updated_at) VALUES ('refs/heads/main','bbbb',1)`));
    rec("update_match", sql.exec(`UPDATE refs SET sha='cccc', updated_at=2 WHERE name='refs/heads/main' AND sha='aaaa'`));
    rec("update_nomatch", sql.exec(`UPDATE refs SET sha='dddd' WHERE name='refs/heads/main' AND sha='zzzz'`));
    rec("changes_after_update_nomatch", sql.exec(`SELECT changes() AS n`));
    rec("update_match_then_changes", sql.exec(`UPDATE refs SET sha='eeee' WHERE name='refs/heads/main' AND sha='cccc'`));
    rec("changes_after_update_match", sql.exec(`SELECT changes() AS n`));
    rec("delete_match", sql.exec(`DELETE FROM refs WHERE name='refs/heads/main' AND sha='eeee'`));
    rec("changes_after_delete", sql.exec(`SELECT changes() AS n`));
    rec("delete_nomatch", sql.exec(`DELETE FROM refs WHERE name='refs/heads/main'`));
    rec("norowid_insert", sql.exec(`INSERT OR IGNORE INTO norowid (name, sha) VALUES ('a','1')`));
    rec("norowid_update_match", sql.exec(`UPDATE norowid SET sha='2' WHERE name='a' AND sha='1'`));
    rec("norowid_delete", sql.exec(`DELETE FROM norowid WHERE name='a' AND sha='2'`));
    // transactionSync with CAS
    const cas = this.ctx.storage.transactionSync(() => {
      sql.exec(`INSERT INTO refs (name, sha, updated_at) VALUES ('refs/heads/x','1111',1)`);
      const c = sql.exec(`UPDATE refs SET sha='2222' WHERE name='refs/heads/x' AND sha='1111'`);
      c.toArray();
      const ch = sql.exec(`SELECT changes() AS n`).one().n;
      return { rowsWritten: c.rowsWritten, changes: ch };
    });
    out["transactionSync_cas"] = cas;
    return out;
  }

  // Q2: does an await on R2 (network) inside the DO let another request interleave?
  async gateNet(id: string) {
    const before = this.counterNet;
    this.log.push(`${id}: read ${before}`);
    await this.env.BUCKET.put(`gate/${id}`, "x"); // network await
    this.counterNet = before + 1;
    this.log.push(`${id}: wrote ${before + 1}`);
    return this.counterNet;
  }
  // Same, but the await is on DO storage (input gate should hold).
  async gateStorage(id: string) {
    const before = this.counterStorage;
    await this.ctx.storage.put(`gs/${id}`, before); // storage await
    this.counterStorage = before + 1;
    return this.counterStorage;
  }
  // Same as gateNet but with a synchronous SQL compare-and-swap after the network await.
  async gateCas(id: string) {
    const sql = this.ctx.storage.sql;
    const cur = sql.exec(`SELECT v FROM kv WHERE k='cas'`).toArray()[0]?.v ?? 0;
    await this.env.BUCKET.put(`gate/cas/${id}`, "x");
    const c = sql.exec(`UPDATE kv SET v=? WHERE k='cas' AND v=?`, cur + 1, cur);
    c.toArray();
    const changed = sql.exec(`SELECT changes() AS n`).one().n as number;
    return { id, sawBefore: cur, casWon: changed === 1 };
  }
  async resetGate() {
    this.counterNet = 0; this.counterStorage = 0; this.log = [];
    this.ctx.storage.sql.exec(`INSERT OR REPLACE INTO kv (k, v) VALUES ('cas', 0)`);
  }
  async gateLog() { return { counterNet: this.counterNet, counterStorage: this.counterStorage, log: this.log,
    cas: this.ctx.storage.sql.exec(`SELECT v FROM kv WHERE k='cas'`).toArray()[0]?.v }; }

  // Q3: is ctx.id.name populated inside a DO created via idFromName?
  async idName() { return { name: this.ctx.id.name ?? null, idString: this.ctx.id.toString() }; }

  // Q4: one alarm slot: set two alarms, which fires?
  async alarmTest() {
    await this.ctx.storage.put("alarmFired", []);
    await this.ctx.storage.setAlarm(Date.now() + 500);
    await this.ctx.storage.put("alarmSet", ["A@+500"]);
    await this.ctx.storage.setAlarm(Date.now() + 1500);
    const set = (await this.ctx.storage.get<string[]>("alarmSet")) ?? [];
    set.push("B@+1500");
    await this.ctx.storage.put("alarmSet", set);
    return { scheduled: await this.ctx.storage.getAlarm() };
  }
  async alarm() {
    const fired = (await this.ctx.storage.get<string[]>("alarmFired")) ?? [];
    fired.push(new Date().toISOString());
    await this.ctx.storage.put("alarmFired", fired);
  }
  async alarmResult() { return { set: await this.ctx.storage.get("alarmSet"), fired: await this.ctx.storage.get("alarmFired") }; }

  // Q5: R2 multipart from inside a DO with 5 MiB parts, then a range read.
  async multipart() {
    const key = "spike/multipart.bin";
    const mpu = await this.env.BUCKET.createMultipartUpload(key);
    const part = new Uint8Array(5 * 1024 * 1024);
    for (let i = 0; i < part.length; i += 4096) part[i] = i & 0xff;
    const p1 = await mpu.uploadPart(1, part);
    const p2 = await mpu.uploadPart(2, part.subarray(0, 1024 * 1024)); // last part may be smaller
    const obj = await mpu.complete([p1, p2]);
    const range = await this.env.BUCKET.get(key, { range: { offset: 5 * 1024 * 1024 - 8, length: 16 } });
    const bytes = range ? new Uint8Array(await range.arrayBuffer()) : null;
    return { size: obj.size, etag: obj.etag, rangeLen: bytes?.length, rangeBytes: bytes ? Array.from(bytes) : null };
  }

  // Q6: subrequest budget from a DO: how many R2 head() calls before failure?
  async subrequests(n: number) {
    let ok = 0; let err: string | null = null;
    try { for (let i = 0; i < n; i++) { await this.env.BUCKET.head(`nope/${i}`); ok++; } }
    catch (e: any) { err = String(e?.message ?? e); }
    return { attempted: n, ok, err };
  }
}

// Q7: node:zlib per-object inflate with consumed-byte reporting.
async function zlibFacts() {
  const out: Record<string, unknown> = {};
  try {
    const zlib = await import("node:zlib");
    out.hasNodeZlib = true;
    // Build two zlib streams back to back, like objects in a packfile.
    const a = zlib.deflateSync(new TextEncoder().encode("blob one: hello hello hello"));
    const b = zlib.deflateSync(new TextEncoder().encode("blob two: world"));
    const joined = new Uint8Array(a.length + b.length); joined.set(a, 0); joined.set(b, a.length);
    // inflateSync with info:true returns { buffer, engine }; engine.bytesWritten is consumed input.
    const r1: any = zlib.inflateSync(joined, { info: true } as any);
    out.inflateSyncInfo = { keys: Object.keys(r1), bytesWritten: r1.engine?.bytesWritten, bytesRead: r1.engine?.bytesRead,
      text: new TextDecoder().decode(r1.buffer), expectedConsumed: a.length };
    const consumed = r1.engine?.bytesWritten;
    if (typeof consumed === "number") {
      const r2: any = zlib.inflateSync(joined.subarray(consumed), { info: true } as any);
      out.secondObject = { text: new TextDecoder().decode(r2.buffer), consumed: r2.engine?.bytesWritten, expected: b.length };
    }
    // Streaming Inflate: does it expose bytesWritten too?
    const inf: any = zlib.createInflate();
    out.streamInflateHasBytesWritten = typeof inf.bytesWritten;
  } catch (e: any) { out.hasNodeZlib = false; out.err = String(e?.stack ?? e); }
  // DecompressionStream('deflate') for comparison: cannot report consumed bytes.
  out.hasDecompressionStream = typeof DecompressionStream !== "undefined";
  return out;
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const stub = env.FACTS.get(env.FACTS.idFromName("owner/repo"));
    switch (url.pathname) {
      case "/": return json({ routes: ["/rows-written", "/zlib", "/gate?n=8", "/id-name", "/alarm", "/alarm-result", "/multipart", "/subrequests?n=1200", "/limits"] });
      case "/rows-written": return json(await stub.rowsWritten());
      case "/zlib": return json(await zlibFacts());
      case "/gate": {
        const n = Number(url.searchParams.get("n") ?? 8);
        await stub.resetGate();
        const net = await Promise.all(Array.from({ length: n }, (_, i) => stub.gateNet(`n${i}`)));
        const sto = await Promise.all(Array.from({ length: n }, (_, i) => stub.gateStorage(`s${i}`)));
        const cas = await Promise.all(Array.from({ length: n }, (_, i) => stub.gateCas(`c${i}`)));
        const after = await stub.gateLog();
        return json({ n, networkAwait: { finalCounter: after.counterNet, lostUpdates: n - after.counterNet, returns: net },
          storageAwait: { finalCounter: after.counterStorage, lostUpdates: n - after.counterStorage },
          casAfterNetworkAwait: { finalValue: after.cas, casWinners: cas.filter(c => c.casWon).length, results: cas }, log: after.log });
      }
      case "/id-name": return json(await stub.idName());
      case "/alarm": return json(await stub.alarmTest());
      case "/alarm-result": return json(await stub.alarmResult());
      case "/multipart": return json(await stub.multipart());
      case "/subrequests": return json(await stub.subrequests(Number(url.searchParams.get("n") ?? 1200)));
      case "/limits": {
        // Worker-level: how many R2 heads from a plain Worker?
        const n = Number(url.searchParams.get("n") ?? 1200); let ok = 0; let err: string | null = null;
        try { for (let i = 0; i < n; i++) { await env.BUCKET.head(`nope/w/${i}`); ok++; } } catch (e: any) { err = String(e?.message ?? e); }
        return json({ attempted: n, ok, err });
      }
      default: return json({ error: "no such route" }, 404);
    }
  },
};
