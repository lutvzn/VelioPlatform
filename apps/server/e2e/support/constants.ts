// What the tests check the server against, as the server has it.

/** How long a closed account is kept before clean-up deletes it (src/closing.rs). */
export const CLOSED_KEEP_MS = 30 * 86_400_000;

/** How many "rev N" messages the test socket (GET /v1/probe/events) sends (src/routes/probe.rs). */
export const EVENT_COUNT = 5;

/** The Permissions-Policy of every page (src/http.rs). */
export const PERMISSIONS =
  "accelerometer=(), camera=(), display-capture=(), geolocation=(), gyroscope=(), hid=(), magnetometer=(), microphone=(), midi=(), payment=(), serial=(), usb=()";
