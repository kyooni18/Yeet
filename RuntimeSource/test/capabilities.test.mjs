import test from "node:test";
import assert from "node:assert/strict";
import { HarnessCapabilityRegistry } from "../dist/capabilities.js";
import { createVisionCapability } from "../dist/vision.js";
import { createLeadCapability } from "../dist/lead.js";

function request(attachedCapabilities) {
  return {
    model: "fake/model",
    messages: [{ role: "user", content: "hello" }],
    ...(attachedCapabilities !== undefined ? { attachedCapabilities } : {}),
  };
}

test("harness capabilities apply defaults but respect an explicit empty attachment list", async () => {
  const calls = [];
  const registry = new HarnessCapabilityRegistry([
    {
      id: "default-module",
      name: "Default",
      description: "default",
      defaultAttached: true,
      prepare(value) {
        calls.push("default-module");
        return { ...value, metadata: { ...(value.metadata ?? {}), default: "yes" } };
      },
    },
  ]);

  const withDefaults = await registry.prepare(request());
  assert.equal(withDefaults.metadata.default, "yes");
  assert.deepEqual(calls, ["default-module"]);

  calls.length = 0;
  const detached = await registry.prepare(request([]));
  assert.equal(detached.metadata, undefined);
  assert.deepEqual(calls, []);
});

test("harness capabilities run only explicitly attached modules in attachment order", async () => {
  const calls = [];
  const module = (id) => ({
    id,
    name: id,
    description: id,
    defaultAttached: false,
    prepare(value) {
      calls.push(id);
      return value;
    },
  });
  const registry = new HarnessCapabilityRegistry([module("one"), module("two")]);

  await registry.prepare(request(["two", "one", "two"]));
  assert.deepEqual(calls, ["two", "one"]);
  assert.rejects(() => registry.prepare(request(["missing"])), /Unknown attached harness capability/);
});

test("vision capability validates supported inline images", async () => {
  const registry = new HarnessCapabilityRegistry([createVisionCapability()]);
  const prepared = await registry.prepare({
    model: "fake/model",
    messages: [{ role: "user", content: "inspect", images: [{ mediaType: "image/png", data: "YWJj" }] }],
  });
  assert.equal(prepared.messages[0].images[0].mediaType, "image/png");
  await assert.rejects(
    () => registry.prepare({
      model: "fake/model",
      messages: [{ role: "user", images: [{ mediaType: "image/tiff", data: "YWJj" }] }],
    }),
    /Unsupported vision image media type/,
  );
});

test("vision capability is default-attached and validates supported inline images", async () => {
  const registry = new HarnessCapabilityRegistry([createVisionCapability()]);
  assert.deepEqual(registry.defaultAttached(), ["vision"]);

  const valid = request();
  valid.messages[0].images = [{ mediaType: "image/png", data: "aGVsbG8=" }];
  await assert.doesNotReject(() => registry.prepare(valid));

  const invalid = request();
  invalid.messages[0].images = [{ mediaType: "image/tiff", data: "aGVsbG8=" }];
  await assert.rejects(() => registry.prepare(invalid), /Unsupported vision image media type/);
});

test("lead capability is opt-in and owns the lead purpose marker", async () => {
  const registry = new HarnessCapabilityRegistry([createLeadCapability()]);
  assert.deepEqual(registry.defaultAttached(), []);

  const ordinary = await registry.prepare(request());
  assert.equal(ordinary.metadata, undefined);

  const lead = await registry.prepare(request(["lead"]));
  assert.equal(lead.metadata?.purpose, "lead");

  const explicitPurpose = await registry.prepare({
    ...request(["lead"]),
    metadata: { purpose: "research" },
  });
  assert.equal(explicitPurpose.metadata?.purpose, "research");
});
