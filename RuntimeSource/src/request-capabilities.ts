import { HarnessCapabilityRegistry } from "./capabilities.js";
import { createContextModeCapability, type RequestCompactorOptions } from "./compact/index.js";
import { withReasoningPolicy } from "./request-policy.js";
import { createVisionCapability } from "./vision.js";

/** Build the request-transform pipeline around provider/runtime dependencies. */
export function createRequestCapabilityRegistry(options: RequestCompactorOptions): HarnessCapabilityRegistry {
  return new HarnessCapabilityRegistry([
    createContextModeCapability({
      contextLength: options.contextLength,
      complete: (request) => options.complete(withReasoningPolicy(request)),
    }),
    createVisionCapability(),
  ]);
}
