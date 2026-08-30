import { createDefaultCore } from "../dist/index.js";

const model = process.env.MODEL;
if (!model) throw new Error("Set MODEL in provider/model form, for example openai/gpt-5");

const core = createDefaultCore();
const result = await core.complete({
  model,
  system: "You are the lead agent in a coding harness.",
  messages: [{ role: "user", content: "Return a one-line status." }],
  timeoutMs: 30_000,
});

console.log(result.text);
