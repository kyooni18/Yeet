import type { SVGIconNode } from '@mun/core'

// Same outline geometry as the compatibility client's conversation icons.
const paths: Record<string, string> = {
  sidebar: 'M4 4h16v16H4zM9 4v16',
  inspector: 'M4 4h16v16H4zM15 4v16',
  chat: 'M21 11.5a8.5 8.5 0 0 1-8.5 8.5H4l-1 1v-9.5a9 9 0 0 1 18 0zM8 9h8M8 13h5',
  plus: 'M12 5v14M5 12h14',
  close: 'M6 6l12 12M18 6 6 18',
  chevron: 'm9 5 7 7-7 7',
  branch: 'M6 6v12M18 6v3a5 5 0 0 1-5 5H6M8 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0M8 20a2 2 0 1 1-4 0 2 2 0 0 1 4 0M20 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0',
  diff: 'M5 5h14v14H5zM8 9h8M12 6v6M8 15h8',
  agents: 'M8 11a3 3 0 1 0 0-6 3 3 0 0 0 0 6M16 10a3 3 0 1 0 0-6M2 20v-2a6 6 0 0 1 12 0v2M17 13a5 5 0 0 1 5 5v2',
  settings: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8M9 3h6l1 3 3 1 2 5-2 5-3 1-1 3H9l-1-3-3-1-2-5 2-5 3-1z',
  arrow: 'M12 19V5M5 12l7-7 7 7',
  expand: 'M14 3h7v7M21 3l-9 9M10 21H3v-7M3 21l9-9',
  refresh: 'M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-2l2 3M4 16l2 3a7 7 0 0 0 12-2',
  check: 'M5 12l4 4L19 6',
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
