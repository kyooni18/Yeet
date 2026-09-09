import type { CallRequest } from "./types.js";
import type { HarnessCapabilityModule } from "./capabilities.js";

export const LEAD_CAPABILITY_ID = "lead";

/**
 * Optional marker for callers that explicitly want a request treated as the
 * primary/lead agent request. This is intentionally not default-attached:
 * ordinary requests should not silently acquire a `purpose: lead` identity.
 */
export function createLeadCapability(): HarnessCapabilityModule {
  return {
    id: LEAD_CAPABILITY_ID,
    name: "Lead Agent",
    description: "Marks an explicitly selected primary-agent request as purpose=lead for request policy and telemetry. Not attached by default.",
    defaultAttached: false,
    prepare(request: CallRequest): CallRequest {
      if (request.metadata?.purpose !== undefined) return request;
      return {
        ...request,
        metadata: { ...(request.metadata ?? {}), purpose: "lead" },
      };
    },
  };
}
