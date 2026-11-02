// Runs static/js/zones.js on frames dumped by test_zones_js.py and prints the result as JSON.
// Usage: node zones_parity.mjs frames.json
import { readFileSync } from "node:fs";
import { analyze } from "./static/js/zones.js";

const frames = JSON.parse(readFileSync(process.argv[2], "utf8"));
const out = {};
for (const [name, f] of Object.entries(frames)) out[name] = analyze(f);
process.stdout.write(JSON.stringify(out));
