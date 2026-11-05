import { describe, expect, it } from "vitest";
import { projectedRunOut } from "./history";
import fixtures from "./pace-fixtures.json";
import type { QuotaSnapshot } from "../types/codex";

// The same cases run against the backend's port (`projected_run_out` in
// src-tauri/src/alerts.rs), so the "runs out" notification and the
// popover's pace line cannot drift apart.
describe("projectedRunOut shared fixtures", () => {
  for (const testCase of fixtures.cases) {
    it(testCase.name, () => {
      const samples: QuotaSnapshot[] = testCase.samples.map((sample) => ({
        timestamp: new Date(fixtures.nowMs - sample.minsAgo * 60_000).toISOString(),
        limitId: "fixture:primary",
        usedPercent: sample.used,
        remainingPercent: 100 - sample.used,
        resetsAt: sample.resetsAt ?? undefined,
      }));
      const bucket = { ...testCase.bucket, resetsAt: testCase.bucket.resetsAt ?? undefined };
      const runOut = projectedRunOut(samples, bucket, fixtures.nowMs);
      if (testCase.expectedRunOutMs === null) {
        expect(runOut).toBeNull();
      } else {
        expect(runOut).not.toBeNull();
        expect(Math.abs((runOut as number) - testCase.expectedRunOutMs)).toBeLessThan(1);
      }
    });
  }
});
