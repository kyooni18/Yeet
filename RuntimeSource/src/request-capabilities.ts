import { HarnessCapabilityRegistry } from "./capabilities.js";
import { createVisionCapability } from "./vision.js";

/** Build the request-transform pipeline around provider/runtime dependencies. */
export function createRequestCapabilityRegistry(): HarnessCapabilityRegistry {
  return new HarnessCapabilityRegistry([
    createVisionCapability(),
  ]);
}
