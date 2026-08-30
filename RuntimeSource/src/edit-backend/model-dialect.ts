import type { ModelDialectRule } from "./types.js";

function globToRegExp(glob: string): RegExp {
  const escaped = glob.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*");
  return new RegExp(`^${escaped}$`, "i");
}

export class ModelDialectSelector {
  readonly defaultDialect: string;
  readonly rules: readonly ModelDialectRule[];

  constructor(defaultDialect = "structured", rules: readonly ModelDialectRule[] = []) {
    this.defaultDialect = defaultDialect;
    this.rules = rules;
  }

  resolve(modelId?: string): string {
    if (!modelId) return this.defaultDialect;
    for (const rule of this.rules) {
      if (globToRegExp(rule.match).test(modelId)) return rule.dialect;
    }
    return this.defaultDialect;
  }
}


