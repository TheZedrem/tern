// `pnpm bench:startup [N]`: launches the release binary with
// `--bench-startup` once cold, then N times warm (default 10), and prints
// the median and p90 per TTFP mark (spec 0001). Build it first with
// `pnpm tauri build --no-bundle`.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

const runs = Number(process.argv[2] ?? 10);
const exe = process.platform === "win32" ? "zed-term.exe" : "zed-term";
const bin = fileURLToPath(new URL(`../src-tauri/target/release/${exe}`, import.meta.url));
const MARKS = ["config", "pty", "window", "first_byte", "rendered"];
const TIMEOUT_MS = 15_000;

if (!Number.isInteger(runs) || runs < 1) {
  console.error("usage: pnpm bench:startup [runs >= 1]");
  process.exit(2);
}
if (!existsSync(bin)) {
  console.error(`No release binary at ${bin}. Run: pnpm tauri build --no-bundle`);
  process.exit(1);
}

/** One launch. spawnSync kills only the process it started if it hangs. */
function launch() {
  const res = spawnSync(bin, ["--bench-startup"], { encoding: "utf8", timeout: TIMEOUT_MS });
  if (res.error) throw new Error(`launch failed: ${res.error.message}`);
  if (res.status !== 0) throw new Error(`exited with ${res.status ?? res.signal}\n${res.stderr}`);
  const line = res.stdout.split("\n").findLast((l) => l.startsWith("{"));
  if (!line) throw new Error(`no report on stdout\n${res.stdout}`);
  return JSON.parse(line);
}

const percentile = (values, p) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)];
};
const fmt = (ms) => (ms == null ? "   n/a" : ms.toFixed(1).padStart(6));

const cold = launch();
const warm = Array.from({ length: runs }, launch);

console.log(`mark          cold   median     p90   (warm, n=${runs})`);
for (const mark of MARKS) {
  const values = warm.map((r) => r[mark]).filter((v) => v != null);
  const med = values.length ? percentile(values, 50) : null;
  const p90 = values.length ? percentile(values, 90) : null;
  console.log(`${mark.padEnd(11)} ${fmt(cold[mark])}   ${fmt(med)}  ${fmt(p90)}`);
}
const ttfp = percentile(warm.map((r) => r.rendered), 50);
console.log(`\nTTFP warm median ${ttfp.toFixed(1)} ms (budget < 300 ms): ${ttfp < 300 ? "OK" : "OVER BUDGET"}`);
