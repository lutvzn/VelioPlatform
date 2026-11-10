// Workers Rate Limiting (wrangler.jsonc, "ratelimits"). Limits count per
// Cloudflare location, so they slow guessing down rather than stop it
// exactly; Argon2id on the owner's device is what makes each guess costly.

export type Limit = "AUTH_LIMIT" | "EMAIL_LIMIT" | "SIGNUP_LIMIT";

/** True if this key may go ahead. Keys are prefixed by what they count, as one binding counts several things. */
export const withinLimit = async (env: Env, limit: Limit, key: string) =>
  (await env[limit].limit({ key })).success;
