export interface TextEnvelope {
    text: string;
    bom: boolean;
    lineEnding: "\n" | "\r\n";
    endsWithNewline: boolean;
}
export declare function decodeText(raw: string): TextEnvelope;
export declare function encodeText(text: string, envelope: Pick<TextEnvelope, "bom" | "lineEnding">): string;
export declare function addressableLines(text: string): string[];
export declare function payloadLines(text: string): string[];
export declare function joinAddressableLines(lines: readonly string[], keepFinalNewline: boolean): string;
export declare function formatNumbered(lines: readonly string[], startLine: number): string;
//# sourceMappingURL=text.d.ts.map