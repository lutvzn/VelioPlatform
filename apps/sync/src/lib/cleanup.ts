// The daily cron (wrangler.jsonc, "triggers").

import { accountStub } from "../account.ts";
import { UNCONFIRMED_MS } from "./accounts.ts";
import { purgeClosedAccounts } from "./closing.ts";

/**
 * The daily cron: unconfirmed accounts after 7 days; closed accounts' data
 * after 30 days; and expired sessions, links and pairings.
 */
export const cleanUp = async (env: Env, now = Date.now()) => {
  const { results } = await env.DB.prepare(
    "SELECT id FROM accounts WHERE confirmed_at IS NULL AND created_at < ? LIMIT 200",
  )
    .bind(now - UNCONFIRMED_MS)
    .all<{ id: string }>();
  for (const { id } of results) {
    await accountStub(env, id).reset();
    await env.DB.prepare("DELETE FROM accounts WHERE id = ? AND confirmed_at IS NULL").bind(id).run();
  }
  const purged = await purgeClosedAccounts(env, now);
  if (purged > 0) console.log(`Deleted the data of ${purged} closed accounts`);
  await env.DB.batch([
    env.DB.prepare("DELETE FROM sessions WHERE expires_at <= ?").bind(now),
    env.DB.prepare("DELETE FROM email_links WHERE expires_at <= ?").bind(now),
    env.DB.prepare("DELETE FROM pairings WHERE expires_at <= ?").bind(now),
    env.DB.prepare("DELETE FROM dev_outbox WHERE sent_at <= ?").bind(now - 7 * 86_400_000),
  ]);
  return results.length;
};
