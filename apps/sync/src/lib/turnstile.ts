// Turnstile on sign-up and reset requests (PLAN.md, "Stack": abuse limits).

import { isRecord } from "@velio/sync-core";
import { isDev } from "./http.ts";

const VERIFY_URL = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/** The form field the Turnstile widget adds. */
export const TURNSTILE_FIELD = "cf-turnstile-response";

/** True if the visitor passed Turnstile. Skipped with DEV_MODE=1; refused when no secret is set. */
export const passedTurnstile = async (env: Env, token: string, ip: string) => {
  if (isDev(env)) return true;
  if (!env.TURNSTILE_SECRET || !token) return false;
  try {
    const response = await fetch(VERIFY_URL, {
      method: "POST",
      body: new URLSearchParams({ secret: env.TURNSTILE_SECRET, response: token, remoteip: ip }),
    });
    if (!response.ok) return false;
    const result: unknown = await response.json();
    return isRecord(result) && result.success === true;
  } catch {
    return false;
  }
};
