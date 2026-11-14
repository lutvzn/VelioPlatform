// Builds and starts the server once for the whole run, on a free port with
// a new database (vitest.config.ts).

import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import type { TestProject } from "vitest/node";

const manifest = path.join(import.meta.dirname, "../../Cargo.toml");

export default async function setup(project: TestProject) {
  const built = spawnSync("cargo", ["build", "--quiet", "--manifest-path", manifest], { stdio: "inherit" });
  if (built.status !== 0) throw new Error("cargo build failed");
  const binary = path.join(path.dirname(manifest), "target/debug/velio-server");

  const dir = mkdtempSync(path.join(tmpdir(), "velio-server-test-"));
  const databasePath = path.join(dir, "velio.db");
  const env = {
    PATH: process.env.PATH ?? "",
    RUST_LOG: process.env.RUST_LOG ?? "warn,velio_server=info",
    NO_COLOR: "1",
    DEV_MODE: "1",
    VELIO_SITE_URL: "https://velio.tv",
    VELIO_LISTEN: "127.0.0.1:0",
    VELIO_DATABASE: databasePath,
    VELIO_TRUSTED_PROXY: "127.0.0.1",
    PRELOGIN_SECRET: "test-prelogin-secret",
    OPS_TOKEN: "test-ops-token",
    EMAIL_FROM: "Velio <no-reply@velio.tv>",
    ABUSE_EMAIL: "abuse@velio.tv",
    // Cloudflare's test keys, so pages carry the widget; DEV_MODE skips the check.
    TURNSTILE_SITE_KEY: "1x00000000000000000000AA",
    TURNSTILE_SECRET: "1x0000000000000000000000000000000AA",
  };
  const server = spawn(binary, [], { env, stdio: ["ignore", "pipe", "inherit"] });

  // The server says where it listens; the log after that is kept for failures.
  const log: string[] = [];
  const serverUrl = await new Promise<string>((resolve, reject) => {
    server.once("exit", code => reject(new Error(`The server stopped (${code}):\n${log.join("")}`)));
    server.stdout.setEncoding("utf8");
    server.stdout.on("data", (chunk: string) => {
      log.push(chunk);
      const match = /Listening on (http:\/\/\S+)/.exec(log.join(""));
      if (match?.[1]) resolve(match[1]);
    });
  });
  if (process.env.VELIO_SERVER_LOG) server.stdout.pipe(process.stdout);

  project.provide("serverUrl", serverUrl);
  project.provide("databasePath", databasePath);
  project.provide("serverBinary", binary);
  project.provide("serverEnv", env);

  return async () => {
    server.kill("SIGTERM");
    await new Promise(resolve => server.once("exit", resolve));
    rmSync(dir, { recursive: true, force: true });
  };
}
