import type { SVGIconNode } from '@mun/core'

// Familiar, legible outline symbols on a consistent 24px grid.
const paths: Record<string, string> = {
  sidebar: 'M4 4h16v16H4zM9 4v16',
  inspector: 'M4 4h16v16H4zM15 4v16',
  chat: 'M5 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-5 3V6a2 2 0 0 1 1-2zM8 9h8M8 13h5',
  plus: 'M12 5v14M5 12h14',
  close: 'M6 6l12 12M18 6 6 18',
  chevron: 'm9 5 7 7-7 7',
  branch: 'M6 6v12M18 6v3a5 5 0 0 1-5 5H6M8 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0M8 20a2 2 0 1 1-4 0 2 2 0 0 1 4 0M20 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0',
  diff: 'M6 3h9l4 4v14H6zM15 3v4h4M9 12h6M12 9v6M9 18h6',
  agents: 'M8 11a3 3 0 1 0 0-6 3 3 0 0 0 0 6M16 10a3 3 0 1 0 0-6M2 20v-2a6 6 0 0 1 12 0v2M17 13a5 5 0 0 1 5 5v2',
  settings: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM10 2h4l.5 2.3 1.8.8 2-1.3 2.8 2.8-1.3 2 .8 1.8 2.4.6v4l-2.4.5-.8 1.8 1.3 2-2.8 2.8-2-1.3-1.8.8L14 22h-4l-.5-2.4-1.8-.8-2 1.3-2.8-2.8 1.3-2-.8-1.8L1 14v-4l2.4-.5.8-1.8-1.3-2 2.8-2.8 2 1.3 1.8-.8z',
  arrow: 'M12 19V5M5 12l7-7 7 7',
  expand: 'M14 3h7v7M21 3l-9 9M10 21H3v-7M3 21l9-9',
  refresh: 'M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-2l2 3M4 16l2 3a7 7 0 0 0 12-2',
  check: 'M5 12l4 4L19 6',
  terminal: 'M3 4h18v16H3zM7 9l3 3-3 3M12 16h5',
  edit: 'M4 20l4-1 13-13-3-3L5 16l-1 4zM15 6l3 3',
  file: 'M5 3h9l5 5v13H5zM14 3v5h5M8 13h8M8 17h8',
  search: 'M15 15l5 5M16.5 10.5a6 6 0 1 1-12 0 6 6 0 0 1 12 0',
  folder: 'M3 6a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
  calendar: 'M5 5h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2zM7 3v4M17 3v4M3 10h18M8 14h.01M12 14h.01M16 14h.01M8 18h.01M12 18h.01',
  debate: 'M4 4h12a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H9l-4 3v-3H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM8 18h7l4 3v-3h1a2 2 0 0 0 2-2v-6a2 2 0 0 0-2-2h-2',
  extensions: 'M4 4h6a2 2 0 1 0 4 0h6v6a2 2 0 1 0 0 4v6h-6a2 2 0 1 0-4 0H4v-6a2 2 0 1 0 0-4z',
  more: 'M5 12h.01M12 12h.01M19 12h.01',
  screen: 'M3 4h18v16H3zM3 9h18M6 6.5h.01M9 6.5h.01',
  web: 'M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0M3 12h18M12 3c-5 5-5 13 0 18 5-5 5-13 0-18',
  mcp: 'M8 3v5M16 3v5M6 8h12v3a6 6 0 0 1-12 0V8M12 17v4',
  skill: 'M12 5C9 3 5 3 3 4v15c3-1 6-1 9 1 3-2 6-2 9-1V4c-3-1-6-1-9 1v15',
  tool: 'm8 7-5 5 5 5M16 7l5 5-5 5M14 4l-4 16',
  activity: 'M4 12h3l3-7 4 14 3-7h3',
  copy: 'M9 9h12v12H9zM15 9V3H3v12h6',
  reasoning: 'M9 18h6M10 21h4M8.5 14.5C7 13.4 6 11.7 6 9.8a6 6 0 1 1 12 0c0 1.9-1 3.6-2.5 4.7-.8.6-1.1 1.3-1.2 2H9.7c-.1-.7-.4-1.4-1.2-2z',
  info: 'M12 11v6M12 7h.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0',
  error: 'M12 7v6M12 17h.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0',
}

const icons = Object.fromEntries(Object.entries(paths).map(([name, d]) => [name, [['path', { d }]] as readonly SVGIconNode[]]))
export function workIcon(name: string): readonly SVGIconNode[] {
  return icons[name] ?? icons.tool
}
