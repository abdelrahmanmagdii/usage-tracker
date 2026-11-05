/**
 * Small deterministic hashing helpers for the simulator. Everything the
 * simulated backend produces is a pure function of (seed, provider, time
 * slot), so a given seed + start time always renders the same app.
 */

/** 32-bit integer mix (lowbias32). */
function mix(x: number): number {
  x = Math.imul(x ^ (x >>> 16), 0x7feb352d);
  x = Math.imul(x ^ (x >>> 15), 0x846ca68b);
  return (x ^ (x >>> 16)) >>> 0;
}

/** Uniform [0, 1) from any number of integer keys. */
export function hash01(...keys: number[]): number {
  let h = 0x9e3779b9;
  for (const key of keys) {
    // Keys can exceed 32 bits (time slots); fold the high part in too.
    const lo = key | 0;
    const hi = Math.floor(key / 4_294_967_296) | 0;
    h = mix(h ^ lo);
    h = mix(h ^ hi ^ 0x632be5ab);
  }
  return h / 4_294_967_296;
}

/** Stable integer for a string (provider ids, scenario names). */
export function hashString(value: string): number {
  let h = 2166136261;
  for (let i = 0; i < value.length; i += 1) {
    h ^= value.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}
