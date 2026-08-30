import { createDefaultCore } from "../dist/index.js";

const model = process.env.MODEL;
if (!model) throw new Error("Set MODEL in provider/model form, for example anthropic/claude-sonnet-4-5");

const core = createDefaultCore();
for await (const event of core.stream({
  model,
  messages: [{ role: "user", content: "Inspect package.json." }],
  tools: [
    {
      name: "read_file",
      description: "Read a UTF-8 file from the workspace",
      inputSchema: {
        type: "object",
        properties: { path: { type: "string" } },
        required: ["path"],
        additionalProperties: false,
      },
    },
  ],
})) {
  if (event.type === "text-delta") process.stdout.write(event.delta);
  if (event.type === "tool-call") console.log("\ntool:", event.toolCall);
}
