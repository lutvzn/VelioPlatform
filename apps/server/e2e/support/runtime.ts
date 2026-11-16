// What the tests reach the server and its database through (globalSetup.ts).
//
//   send(path, init, ip)   a request to the server, as from `ip`: the server
//                          trusts X-Forwarded-For from 127.0.0.1, and a
//                          made-up address such as "10.1.2.3.4" becomes an
//                          IPv6 one of its own, as the server reads only
//                          addresses
//   sql(query)             the rows of a query, as JSON, run with psql as the
//                          database's owner, past every policy
//   lit(text)              text as an SQL literal, for sql()

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { isIP } from "node:net";
import { inject } from "vitest";

export const SITE = "https://velio.tv";
const serverUrl = new URL(inject("serverUrl"));
export const databaseUrl = inject("databaseUrl");

const asAddress = (ip: string) => {
  if (isIP(ip)) return ip;
  const hex = createHash("sha256").update(ip).digest("hex");
  return `fd${hex.slice(0, 2)}:${hex.slice(2, 30).match(/.{4}/g)?.join(":")}`;
};

/** An answer as the tests read it: json() takes the type it reads. */
export interface Answer extends Response {
  json<T>(): Promise<T>;
}

// Only the type changes: json() reads whatever the test says it reads.
// oxlint-disable-next-line typescript/no-unsafe-type-assertion
const asAnswer = (response: Response) => response as Answer;

export const send = async (path: string, init: RequestInit = {}, ip?: string): Promise<Answer> => {
  const headers = new Headers(init.headers);
  if (ip) headers.set("X-Forwarded-For", asAddress(ip));
  const target = new URL(path, serverUrl);
  return asAnswer(await fetch(target, { ...init, headers, redirect: "manual" }));
};

const psql = process.env.PG_BIN ? `${process.env.PG_BIN}/psql` : "psql";

export const lit = (text: string) => `'${text.replaceAll("'", "''")}'`;

/** The rows of a query, each an object keyed by column. */
export const sql = <Row = Record<string, unknown>>(query: string): Row[] => {
  const printed = execFileSync(
    psql,
    [
      databaseUrl,
      "-XAtq",
      "-v",
      "ON_ERROR_STOP=1",
      "-c",
      `SELECT coalesce(json_agg(r), '[]'::json) FROM (${query}) r`,
    ],
    { encoding: "utf8" },
  );
  // psql prints what Postgres made of the query: JSON, of the rows asked for.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  return JSON.parse(printed) as Row[];
};

/** The first row's first value, or null. */
export const value = (query: string): unknown => {
  const row = sql(query)[0];
  return row ? Object.values(row)[0] : null;
};

/** Runs a statement that answers no rows. */
export const exec = (statement: string) => {
  execFileSync(psql, [databaseUrl, "-XAtq", "-v", "ON_ERROR_STOP=1", "-c", statement], { encoding: "utf8" });
};

declare module "vitest" {
  export interface ProvidedContext {
    serverUrl: string;
    databaseUrl: string;
    serverBinary: string;
    serverEnv: Record<string, string>;
  }
}
