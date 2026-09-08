import type { HarnessCapabilityModule } from "../capabilities.js";
import { RequestCompactor, type RequestCompactorOptions } from "./requestCompactor.js";

export const CONTEXT_MODE_CAPABILITY_ID = "context-mode";

export function createContextModeCapability(options: RequestCompactorOptions): HarnessCapabilityModule {
  const compactor = new RequestCompactor(options);
  return {
    id: CONTEXT_MODE_CAPABILITY_ID,
    name: "Context Mode",
    description: "Compacts older conversation and tool history to preserve useful context within the model window.",
    defaultAttached: true,
    prepare: (request) => compactor.compact(request),
    prepareWithUsage: (request) => compactor.compactDetailed(request),
  };
}
