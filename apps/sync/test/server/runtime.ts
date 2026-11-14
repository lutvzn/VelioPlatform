// Stands in for `cloudflare:test` and `cloudflare:workers` when the tests run
// against the Rust server (vitest.server.config.ts; PLAN.md, "Moving off
// Cloudflare", "Checking it matches"), so the test files stay as they are.
//
//   exports.default.fetch  sends the request to the server: any origin becomes
//                          the server's, and cf-connecting-ip becomes
//                          X-Forwarded-For, which the server trusts from 127.0.0.1
//                          (a made-up one such as "10.1.2.3.4" becomes an IPv6
//                          address of its own, as the server reads only addresses)
//   env.DB                 a D1-like reader and writer on the server's SQLite file
//   runInDurableObject     an account's storage.sql, as the vault_* tables'
//                          rows for that account
//
// account.ts and cleanup.ts stand in for the Worker modules the tests import.
//
// A request with `Upgrade: websocket` opens a real socket and answers 101
// with it as `webSocket`, as a Worker's fetch does.

import { createHash } from "node:crypto";
import { request as httpRequest } from "node:http";
import { isIP } from "node:net";
import { DatabaseSync, type SQLInputValue, type SQLOutputValue } from "node:sqlite";
import { inject } from "vitest";

const serverUrl = new URL(inject("serverUrl"));

const asAddress = (ip: string) => {
  if (isIP(ip)) return ip;
  const hex = createHash("sha256").update(ip).digest("hex");
  return `fd${hex.slice(0, 2)}:${hex.slice(2, 30).match(/.{4}/g)?.join(":")}`;
};

const toServer = (input: string | URL | Request, init: RequestInit = {}) => {
  const source = input instanceof Request ? input : new Request(input, init);
  const url = new URL(source.url);
  const target = new URL(url.pathname + url.search, serverUrl);
  const headers = new Headers(source.headers);
  const ip = headers.get("cf-connecting-ip");
  headers.delete("cf-connecting-ip");
  if (ip) headers.set("X-Forwarded-For", asAddress(ip));
  return { source, target, headers };
};

const openSocket = async (target: URL, headers: Headers): Promise<Response> => {
  const wsUrl = new URL(target);
  wsUrl.protocol = target.protocol === "https:" ? "wss:" : "ws:";
  const protocols = (headers.get("Sec-WebSocket-Protocol") ?? "")
    .split(",")
    .map(protocol => protocol.trim())
    .filter(Boolean);
  const sent: Record<string, string> = {};
  for (const [name, value] of headers) {
    if (!["upgrade", "connection", "sec-websocket-protocol"].includes(name)) sent[name] = value;
  }
  // Node's WebSocket (undici) takes headers as well as protocols.
  const socket = new WebSocket(wsUrl, { protocols, headers: sent });
  const opened = await new Promise<boolean>(resolve => {
    socket.addEventListener("open", () => resolve(true), { once: true });
    socket.addEventListener("error", () => resolve(false), { once: true });
  });
  if (!opened) return refusedUpgrade(target, sent, protocols);
  // A Worker's fetch answers an upgrade with a 101 and the socket, which it
  // has to accept; this one is open already. Response can't be made with a 101.
  // The protocol the server picked comes back as the Worker's header did.
  const answerHeaders = socket.protocol ? { "Sec-WebSocket-Protocol": socket.protocol } : undefined;
  const answer = Object.assign(new Response(null, { headers: answerHeaders }), {
    webSocket: Object.assign(socket, { accept() {} }),
  });
  Object.defineProperty(answer, "status", { value: 101 });
  return answer;
};

/** The socket wasn't opened: asks again over plain HTTP, to see the server's answer. */
const refusedUpgrade = (target: URL, headers: Record<string, string>, protocols: string[]) =>
  new Promise<Response>((resolve, reject) => {
    const req = httpRequest(target, {
      headers: {
        ...headers,
        Connection: "Upgrade",
        Upgrade: "websocket",
        "Sec-WebSocket-Version": "13",
        "Sec-WebSocket-Key": Buffer.from(crypto.getRandomValues(new Uint8Array(16))).toString("base64"),
        ...(protocols.length ? { "Sec-WebSocket-Protocol": protocols.join(", ") } : {}),
      },
    });
    req.on("upgrade", (_res, socket) => {
      socket.destroy();
      reject(new Error("The server took the socket on the second try only"));
    });
    req.on("response", res => {
      const chunks: Buffer[] = [];
      res.on("data", (chunk: Buffer) => chunks.push(chunk));
      res.on("end", () => {
        const answerHeaders = new Headers();
        for (const [name, value] of Object.entries(res.headers)) {
          for (const one of [value ?? []].flat()) answerHeaders.append(name, one);
        }
        resolve(new Response(Buffer.concat(chunks), { status: res.statusCode, headers: answerHeaders }));
      });
    });
    req.on("error", reject);
    req.end();
  });

const fetchFromServer = async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
  const { source, target, headers } = toServer(input, init);
  if (headers.get("Upgrade")?.toLowerCase() === "websocket") return openSocket(target, headers);
  const body = source.body ? await source.arrayBuffer() : undefined;
  return fetch(target, { method: source.method, headers, body, redirect: "manual" });
};

export const exports = { default: { fetch: fetchFromServer } };

// --- env.DB ---

export const db = new DatabaseSync(inject("databasePath"), { timeout: 5_000 });

// The tests' types are D1's, from the Workers types; this only has to act alike.
const plain = (row: Record<string, SQLOutputValue> | undefined) => (row ? { ...row } : null);

class Statement {
  constructor(
    private readonly sql: string,
    private readonly values: SQLInputValue[] = [],
  ) {}

  bind(...values: SQLInputValue[]) {
    return new Statement(this.sql, values);
  }

  async first(column?: string) {
    const row = plain(db.prepare(this.sql).get(...this.values));
    return column === undefined ? row : (row?.[column] ?? null);
  }

  async all() {
    const results = db
      .prepare(this.sql)
      .all(...this.values)
      .map(row => plain(row));
    return { results, success: true };
  }

  async run() {
    const { changes } = db.prepare(this.sql).run(...this.values);
    return { success: true, meta: { changes: Number(changes) } };
  }
}

export const env = { DB: { prepare: (sql: string) => new Statement(sql) } };

// --- runInDurableObject ---

/** What a Durable Object's `storage.sql` answers, for the two queries the tests make of it. */
const accountSql = (accountId: string) => ({
  exec(sql: string) {
    const rows = (): Record<string, SQLOutputValue>[] => {
      if (sql.startsWith("SELECT name FROM sqlite_master WHERE type = 'table'"))
        return db
          .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'vault\\_%' ESCAPE '\\'",
          )
          .all();
      const table = /^SELECT \* FROM (vault_\w+)$/.exec(sql)?.[1];
      if (table) return db.prepare(`SELECT * FROM ${table} WHERE account_id = ?`).all(accountId);
      throw new Error(`The stand-in can't run this on an account: ${sql}`);
    };
    return { toArray: () => rows().map(row => ({ ...row })) };
  },
});

export const runInDurableObject = async <Stub extends { accountId: string }, Result>(
  stub: Stub,
  run: (instance: Stub, state: { storage: { sql: ReturnType<typeof accountSql> } }) => Result,
) => run(stub, { storage: { sql: accountSql(stub.accountId) } });

declare module "vitest" {
  export interface ProvidedContext {
    serverUrl: string;
    databasePath: string;
    serverBinary: string;
    serverEnv: Record<string, string>;
  }
}
