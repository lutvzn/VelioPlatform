// What the vault pages need from an account, and the TVs signed in to it.

import { storedManifest } from "@velio/sync-core";
import { accountStub, type ChangeResult } from "../account.ts";
import type { AppContext, Session } from "./http.ts";
import type { VaultPageProps } from "../views/vault.tsx";

export const NO_KEYS = "This account's keys are missing. Reset the account to start again.";

export const vaultProps = async (
  c: AppContext,
  session: Pick<Session, "accountId" | "email">,
): Promise<VaultPageProps | null> => {
  const stub = accountStub(c.env, session.accountId);
  const [owner, stored, devices] = await Promise.all([
    stub.getOwner(),
    stub.getState(),
    devicesOf(c.env.DB, session.accountId),
  ]);
  if (!owner) return null;
  return {
    accountId: session.accountId,
    email: session.email,
    signPublicKey: owner.signPublicKey,
    stored,
    manifest: storedManifest(stored, session.accountId),
    devices,
  };
};

/** Why the Durable Object refused a change, in words for the owner. */
export const refusalText = (result: Exclude<ChangeResult, { ok: true }>) => {
  if (result.code === "behind")
    return "Your lists were changed in another tab or on another device. Reload the page to see the changes, then make yours again.";
  if (result.code === "limit") return `That's past the limits for an account: ${result.message}.`;
  return "Velio couldn't take this change. Reload the page and try again.";
};

export interface DeviceRow {
  id: string;
  name: string;
  created_at: number;
  last_pulled_at: number | null;
  /** Set when the TV signed itself out (POST /v1/signout); it stays in the manifest until removed. */
  signed_out_at: number | null;
}

const day = (time: number) => new Date(time).toISOString().slice(0, 10);

/** The line under a TV's name on the account page and the TVs page. */
export const deviceStatus = (device: DeviceRow) =>
  device.signed_out_at === null
    ? `Signed in ${day(device.created_at)}` +
      (device.last_pulled_at === null ? "" : `, last synced ${day(device.last_pulled_at)}`)
    : `Signed out on the TV ${day(device.signed_out_at)}. Remove it to take it off the account.`;

/** Signs out TVs a change removed: without their D1 rows, their tokens open nothing. */
export const forgetDevices = async (db: D1Database, accountId: string, deviceIds: readonly string[]) => {
  if (deviceIds.length === 0) return;
  await db.batch(
    deviceIds.map(deviceId =>
      db.prepare("DELETE FROM devices WHERE id = ? AND account_id = ?").bind(deviceId, accountId),
    ),
  );
};

export const devicesOf = async (db: D1Database, accountId: string) =>
  (
    await db
      .prepare(
        "SELECT id, name, created_at, last_pulled_at, signed_out_at FROM devices WHERE account_id = ? ORDER BY created_at",
      )
      .bind(accountId)
      .all<DeviceRow>()
  ).results;
