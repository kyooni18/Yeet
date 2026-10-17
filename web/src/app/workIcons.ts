import type { SVGIconNode } from '@mun/core'

// Same outline geometry as the compatibility client's conversation icons.
const paths: Record<string, string> = {
  terminal: 'M3 4h18v16H3zM7 9l3 3-3 3M12 16h5',
  edit: 'M4 20l4-1 13-13-3-3L5 16l-1 4zM15 6l3 3',
  file: 'M14 2H6v20h12V6zM14 2v6h6M8 13h8M8 17h8',
  search: 'M15 15l5 5M16.5 10.5a6 6 0 1 1-12 0 6 6 0 0 1 12 0',
  folder: 'M3.5 7.5h6l2-2h9v13h-17z',
  screen: 'M3 4h18v16H3zM3 9h18M6 6.5h.01M9 6.5h.01',
  web: 'M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0M3 12h18M12 3c-5 5-5 13 0 18 5-5 5-13 0-18',
  mcp: 'M8 3v5M16 3v5M6 8h12v3a6 6 0 0 1-12 0V8M12 17v4',
  skill: 'M12 5C9 3 5 3 3 4v15c3-1 6-1 9 1 3-2 6-2 9-1V4c-3-1-6-1-9 1v15',
  tool: 'M14 6a5 5 0 0 0-6 6L3 17a3 3 0 0 0 4 4l5-5a5 5 0 0 0 6-6l-3 3-4-4 3-3z',
  reasoning: 'M9 18h6M10 21h4M8.5 14.5C7 13.4 6 11.7 6 9.8a6 6 0 1 1 12 0c0 1.9-1 3.6-2.5 4.7-.8.6-1.1 1.3-1.2 2H9.7c-.1-.7-.4-1.4-1.2-2z',
  info: 'M12 11v6M12 7h.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0',
  error: 'M12 7v6M12 17h.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0',
}

const icons = Object.fromEntries(Object.entries(paths).map(([name, d]) => [name, [['path', { d }]] as readonly SVGIconNode[]]))
export function workIcon(name: string): readonly SVGIconNode[] {
  return icons[name] ?? icons.tool
}
