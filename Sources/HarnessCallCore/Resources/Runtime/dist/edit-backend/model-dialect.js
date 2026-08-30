function globToRegExp(glob) {
    const escaped = glob.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*");
    return new RegExp(`^${escaped}$`, "i");
}
export class ModelDialectSelector {
    defaultDialect;
    rules;
    constructor(defaultDialect = "structured", rules = []) {
        this.defaultDialect = defaultDialect;
        this.rules = rules;
    }
    resolve(modelId) {
        if (!modelId)
            return this.defaultDialect;
        for (const rule of this.rules) {
            if (globToRegExp(rule.match).test(modelId))
                return rule.dialect;
        }
        return this.defaultDialect;
    }
}
//# sourceMappingURL=model-dialect.js.map