import type { HarnessCapabilityModule } from "./capabilities.js";
import type { CallRequest, ImageAttachment } from "./types.js";

export const VISION_CAPABILITY_ID = "vision";

const SUPPORTED_MEDIA_TYPES = new Set<ImageAttachment["mediaType"]>([
  "image/png",
  "image/jpeg",
  "image/webp",
  "image/gif",
]);

function validateImage(image: ImageAttachment): void {
  if (!SUPPORTED_MEDIA_TYPES.has(image.mediaType)) {
    throw new TypeError(`Unsupported vision image media type: ${String(image.mediaType)}`);
  }
  if (!image.data || !/^[A-Za-z0-9+/]*={0,2}$/.test(image.data)) {
    throw new TypeError("Vision image data must be non-empty base64");
  }
}

/** Validate image inputs automatically; vision is not a user-toggleable capability. */
export function prepareVisionRequest(request: CallRequest): CallRequest {
  for (const message of request.messages) {
    for (const image of message.images ?? []) validateImage(image);
  }
  return request;
}

export function createVisionCapability(): HarnessCapabilityModule {
  return {
    id: VISION_CAPABILITY_ID,
    name: "Vision",
    description: "Allows PNG, JPEG, WebP, or GIF images to be attached to model requests for multimodal models.",
    // Unlike web search, vision has no background process or tool-schema cost.
    // Compatibility wrapper for embedders; Yeet validates image requests automatically.
    defaultAttached: true,
    prepare: prepareVisionRequest,
  };
}
