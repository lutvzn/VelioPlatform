// A TV signing in as Nuvio's TV app does: it asks for a code
// (start_tv_login_session), shows its QR code, a signed-in browser approves
// it on /tv-login, and the TV exchanges the code for a session
// (tv-logins-exchange), then registers itself (register_current_device).

import { randomBytes } from "node:crypto";
import { expect } from "vitest";
import {
  api,
  fresh,
  post,
  request,
  rpc,
  signedInAccount,
  SITE,
  type Tokens,
  type Visitor,
} from "./helpers.ts";

export interface Tv {
  ip: string;
  nonce: string;
  code: string;
}

/** The TV asks for a code, and gets the page its QR code opens. */
export const startTvLogin = async (name = "Living room TV"): Promise<Tv & { webUrl: string }> => {
  const { ip } = fresh();
  const nonce = randomBytes(16).toString("hex");
  const started = await rpc(
    "start_tv_login_session",
    { p_device_nonce: nonce, p_redirect_base_url: `${SITE}/tv-login`, p_device_name: name },
    undefined,
    ip,
  );
  expect(started.status).toBe(200);
  const [row] = await started.json<{ code: string; web_url: string }[]>();
  if (!row) throw new Error("No code");
  return { ip, nonce, code: row.code, webUrl: row.web_url };
};

/**
 * The TV asks for a code to type as well (start_device_login_session), as
 * VelioTV does: its QR code opens /link with the code, and the code is
 * typed there otherwise. It polls and exchanges the device code.
 */
export const startDeviceLogin = async (name = "Living room TV") => {
  const { ip } = fresh();
  const nonce = randomBytes(16).toString("hex");
  const started = await rpc(
    "start_device_login_session",
    { p_device_nonce: nonce, p_redirect_base_url: `${SITE}/link`, p_device_name: name, p_device_type: "tv" },
    undefined,
    ip,
  );
  expect(started.status).toBe(200);
  const [row] = await started.json<
    {
      device_code: string;
      user_code: string;
      verification_uri: string;
      verification_uri_complete: string;
    }[]
  >();
  if (!row) throw new Error("No code");
  return { ip, nonce, code: row.device_code, userCode: row.user_code, row };
};

/** What the TV's poll says: pending, approved, expired or used. */
export const poll = async (tv: Tv) => {
  const answer = await rpc(
    "poll_tv_login_session",
    { p_code: tv.code, p_device_nonce: tv.nonce },
    undefined,
    tv.ip,
  );
  expect(answer.status).toBe(200);
  return (await answer.json<{ status: string }[]>())[0]?.status;
};

export const exchange = (tv: Tv) =>
  api("/functions/v1/tv-logins-exchange", { body: { code: tv.code, device_nonce: tv.nonce }, ip: tv.ip });

/** The browser approves the TV's code on /tv-login. */
export const approve = async (browser: Visitor, tv: Tv) => {
  const page = await request(browser, `/tv-login?code=${tv.code}`);
  expect(page.status).toBe(200);
  return post(browser, "/tv-login", { code: tv.code });
};

/** A TV signed in and registered, as Nuvio TV does after the exchange. */
export const signInTv = async (browser: Visitor, name = "Living room TV") => {
  const tv = await startTvLogin(name);
  expect((await approve(browser, tv)).status).toBe(200);
  const exchanged = await exchange(tv);
  expect(exchanged.status).toBe(200);
  const tokens = await exchanged.json<Tokens>();
  const registered = await rpc(
    "register_current_device",
    {
      p_installation_id: `tv-${tv.nonce}`,
      p_client_name: "Nuvio TV",
      p_client_version: "test",
      p_platform: "tv",
      p_device_name: name,
    },
    tokens.access_token,
    tv.ip,
  );
  expect(registered.status).toBe(200);
  return { ...tv, tokens };
};

/** An account with a browser and a TV signed in. */
export const signedInTv = async () => {
  const account = await signedInAccount();
  const tv = await signInTv(account.browser);
  return { ...account, tv };
};

/**
 * Whether the TV is still signed in: its session, as Auth checks it. (Its
 * access token still reads REST until it expires, as with PostgREST; its
 * refresh token is what stops working.)
 */
export const signedIn = async (tv: Tv & { tokens: Tokens }) =>
  (await api("/auth/v1/user", { token: tv.tokens.access_token, ip: tv.ip })).status === 200;
