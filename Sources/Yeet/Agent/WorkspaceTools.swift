import HarnessCallCore

enum WorkspaceTools {
    static let definitions: [ToolDefinition] = [
        ToolDefinition(
            name: "read_file",
            description: "Read a UTF-8 text file in the workspace. The returned snapshot must be supplied to later edits.",
            inputSchema: [
                "type": "object",
                "properties": [
                    "path": ["type": "string", "description": "Workspace-relative file path."],
                    "startLine": ["type": "integer", "minimum": 1],
                    "endLine": ["type": "integer", "minimum": 1]
                ],
                "required": ["path"],
                "additionalProperties": false
            ]
        ),
        ToolDefinition(
            name: "apply_file_edits",
            description: "Apply snapshot-safe structured edits or file operations in one transaction.",
            inputSchema: [
                "type": "object",
                "properties": [
                    "changes": [
                        "type": "array",
                        "items": [
                            "type": "object",
                            "properties": [
                                "path": ["type": "string"],
                                "snapshot": ["type": "string"],
                                "edits": [
                                    "type": "array",
                                    "items": [
                                        "type": "object",
                                        "properties": [
                                            "kind": ["type": "string", "enum": ["replace", "delete", "insert", "replaceBlock", "insertAfterBlock", "deleteBlock"]],
                                            "range": ["type": "object", "properties": ["start": ["type": "integer"], "end": ["type": "integer"]]],
                                            "at": ["type": "object", "properties": ["kind": ["type": "string", "enum": ["start", "end", "before", "after"]], "line": ["type": "integer"]]],
                                            "line": ["type": "integer"],
                                            "text": ["type": "string"]
                                        ],
                                        "required": ["kind"]
                                    ]
                                ],
                                "fileOp": [
                                    "type": "object",
                                    "properties": [
                                        "kind": ["type": "string", "enum": ["create", "delete", "move"]],
                                        "text": ["type": "string"],
                                        "mode": ["type": "integer"],
                                        "destination": ["type": "string"]
                                    ],
                                    "required": ["kind"]
                                ]
                            ],
                            "required": ["path"]
                        ],
                        "minItems": 1
                    ],
                    "diagnostics": ["type": "boolean"]
                ],
                "required": ["changes"],
                "additionalProperties": false
            ]
        )
    ]
}
