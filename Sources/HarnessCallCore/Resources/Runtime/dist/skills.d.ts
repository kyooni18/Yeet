export interface SkillSummary {
    name: string;
    description: string;
    root: string;
    entrypoint: string;
}
export interface Skill extends SkillSummary {
    instructions: string;
    files: string[];
}
export interface SkillRegistryOptions {
    configDir?: string;
    roots?: string[];
}
export declare class SkillRegistry {
    readonly configDir: string;
    readonly roots: string[];
    constructor(options?: SkillRegistryOptions);
    ensure(): Promise<void>;
    list(): Promise<SkillSummary[]>;
    load(name: string): Promise<Skill>;
    readSkillFile(name: string, path: string): Promise<string>;
}
//# sourceMappingURL=skills.d.ts.map