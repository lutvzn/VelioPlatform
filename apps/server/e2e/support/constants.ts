// What the tests check the server against, as the server has it.

/** How long a closed account is kept before clean-up deletes it (src/closing.rs). */
export const CLOSED_KEEP_MS = 30 * 86_400_000;

/** How long an unconfirmed account is kept (src/cleanup.rs). */
export const UNCONFIRMED_KEEP_MS = 7 * 86_400_000;

/** The Permissions-Policy of every page (src/http.rs). */
export const PERMISSIONS =
  "accelerometer=(), camera=(), display-capture=(), geolocation=(), gyroscope=(), hid=(), magnetometer=(), microphone=(), midi=(), payment=(), serial=(), usb=()";
