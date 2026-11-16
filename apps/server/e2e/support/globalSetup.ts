// Builds the server and starts it once for the whole run, on a free port,
// with a database of its own (vitest.config.ts):
//
//   VELIO_TEST_DATABASE_URL set   a new database is made on that server (its
//                                 user must be a superuser) and dropped after
//   not set                       a throwaway Postgres cluster is made with
//                                 initdb in a temporary folder, on a free port,
//                                 and deleted after (initdb and postgres on
//                                 PATH, or in PG_BIN)
//
// Sign-ups are confirmed by email here (ENABLE_EMAIL_AUTOCONFIRM=false), so
// the links are tested; nuvio.test.ts starts a second server on the same
// database that confirms them at once, as Nuvio's smoke tests need.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import type { TestProject } from "vitest/node";

const manifest = path.join(import.meta.dirname, "../../Cargo.toml");

const bin = (name: string) => (process.env.PG_BIN ? path.join(process.env.PG_BIN, name) : name);

export const freePort = () =>
  new Promise<number>((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => (typeof address === "object" && address ? resolve(address.port) : reject()));
    });
  });

const run = (command: string, args: string[]) => {
  const done = spawnSync(command, args, { encoding: "utf8" });
  if (done.error) throw new Error(`${command}: ${done.error.message}`);
  if (done.status !== 0) throw new Error(`${command} ${args.join(" ")} failed:\n${done.stderr}`);
  return done.stdout;
};

/** Waits until the cluster answers, or its process stops. */
const ready = async (url: string, postgres: ChildProcess) => {
  for (let tries = 0; tries < 100; tries++) {
    if (postgres.exitCode !== null) throw new Error("Postgres stopped while starting");
    if (spawnSync(bin("psql"), [url, "-XAtqc", "SELECT 1"]).status === 0) return;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error("Postgres didn't start");
};

interface Database {
  url: string;
  stop(): Promise<void>;
}

/** A new database on the server VELIO_TEST_DATABASE_URL names. */
const onExistingServer = (admin: string): Database => {
  const name = `velio_e2e_${randomBytes(6).toString("hex")}`;
  run(bin("psql"), [admin, "-XAtqc", `CREATE DATABASE ${name}`]);
  const url = new URL(admin);
  url.pathname = `/${name}`;
  return {
    url: url.toString(),
    async stop() {
      run(bin("psql"), [admin, "-XAtqc", `DROP DATABASE IF EXISTS ${name} WITH (FORCE)`]);
    },
  };
};

/** A throwaway cluster of its own. */
const ownCluster = async (dir: string): Promise<Database> => {
  const data = path.join(dir, "pg");
  run(bin("initdb"), ["-D", data, "-U", "postgres", "--auth=trust", "-E", "UTF8", "--locale=C", "--no-sync"]);
  const port = await freePort();
  const postgres = spawn(
    bin("postgres"),
    ["-D", data, "-p", String(port), "-k", dir, "-c", "listen_addresses=127.0.0.1", "-c", "fsync=off"],
    { stdio: ["ignore", "ignore", process.env.VELIO_SERVER_LOG ? "inherit" : "ignore"] },
  );
  const url = `postgres://postgres@127.0.0.1:${port}/postgres`;
  await ready(url, postgres);
  return {
    url,
    async stop() {
      if (postgres.exitCode !== null) return;
      postgres.kill("SIGINT");
      await new Promise(resolve => postgres.once("exit", resolve));
    },
  };
};

/** Starts the server, and answers once it says where it listens. */
export const startServer = async (binary: string, env: Record<string, string>) => {
  const server = spawn(binary, [], { env, stdio: ["ignore", "pipe", "inherit"] });
  const log: string[] = [];
  const url = await new Promise<string>((resolve, reject) => {
    server.once("exit", code => reject(new Error(`The server stopped (${code}):\n${log.join("")}`)));
    server.stdout.setEncoding("utf8");
    server.stdout.on("data", (chunk: string) => {
      log.push(chunk);
      const match = /Listening on (http:\/\/\S+)/.exec(log.join(""));
      if (match?.[1]) resolve(match[1]);
    });
  });
  if (process.env.VELIO_SERVER_LOG) server.stdout.pipe(process.stdout);
  return {
    url,
    async stop() {
      if (server.exitCode !== null) return;
      server.kill("SIGTERM");
      await new Promise(resolve => server.once("exit", resolve));
    },
  };
};

export default async function setup(project: TestProject) {
  const built = spawnSync("cargo", ["build", "--quiet", "--manifest-path", manifest], { stdio: "inherit" });
  if (built.status !== 0) throw new Error("cargo build failed");
  const binary = path.join(path.dirname(manifest), "target/debug/velio-server");

  const dir = mkdtempSync(path.join(tmpdir(), "velio-server-test-"));
  const admin = process.env.VELIO_TEST_DATABASE_URL;
  const database = admin ? onExistingServer(admin) : await ownCluster(dir);
  const env = {
    PATH: process.env.PATH ?? "",
    RUST_LOG: process.env.RUST_LOG ?? "warn,velio_server=info",
    NO_COLOR: "1",
    DEV_MODE: "1",
    VELIO_SITE_URL: "https://velio.tv",
    VELIO_LISTEN: "127.0.0.1:0",
    DATABASE_URL: database.url,
    VELIO_STORAGE_DIR: path.join(dir, "storage"),
    VELIO_TRUSTED_PROXY: "127.0.0.1",
    JWT_SECRET: "test-jwt-secret-of-at-least-32-characters",
    ENABLE_EMAIL_AUTOCONFIRM: "false",
    OPS_TOKEN: "test-ops-token",
    EMAIL_FROM: "Velio <no-reply@velio.tv>",
    ABUSE_EMAIL: "abuse@velio.tv",
    // Cloudflare's test keys, so pages carry the widget; DEV_MODE skips the check.
    TURNSTILE_SITE_KEY: "1x00000000000000000000AA",
    TURNSTILE_SECRET: "1x0000000000000000000000000000000AA",
  };
  let server;
  try {
    server = await startServer(binary, env);
  } catch (error) {
    await database.stop();
    throw error;
  }

  project.provide("serverUrl", server.url);
  project.provide("databaseUrl", database.url);
  project.provide("serverBinary", binary);
  project.provide("serverEnv", env);

  return async () => {
    await server.stop();
    await database.stop();
    rmSync(dir, { recursive: true, force: true });
  };
}
