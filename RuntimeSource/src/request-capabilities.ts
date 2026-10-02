import { HarnessCapabilityRegistry } from "./capabilities.js";

/** Build the request-transform pipeline around provider/runtime dependencies. */
export function createRequestCapabilityRegistry(): HarnessCapabilityRegistry {
  return new HarnessCapabilityRegistry();
}
