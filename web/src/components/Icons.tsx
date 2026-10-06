import type { ReactNode, SVGProps } from 'react'

export type IconProps = Omit<SVGProps<SVGSVGElement>, 'width' | 'height'> & {
  size?: number | string
  strokeWidth?: number
}

function Icon({
  size = 20,
  strokeWidth = 1.7,
  children,
  ...props
}: IconProps & { children: ReactNode }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...props}
    >
      {children}
    </svg>
  )
}

export const X = (p: IconProps) => <Icon {...p}><path d="M6 6l12 12M18 6L6 18" /></Icon>
export const Plus = (p: IconProps) => <Icon {...p}><path d="M12 5v14M5 12h14" /></Icon>
export const Check = (p: IconProps) => <Icon {...p}><path d="M5 12.5l4.2 4.2L19 7.5" /></Icon>
export const ChevronRight = (p: IconProps) => <Icon {...p}><path d="M9 6l6 6-6 6" /></Icon>
export const ChevronDown = (p: IconProps) => <Icon {...p}><path d="M6 9l6 6 6-6" /></Icon>
export const ChevronsUpDown = (p: IconProps) => <Icon {...p}><path d="M8 9l4-4 4 4M8 15l4 4 4-4" /></Icon>
export const Send = (p: IconProps) => <Icon {...p}><path d="M4 4l17 8-17 8 3-8-3-8zM7 12h14" /></Icon>
export const Square = (p: IconProps) => <Icon {...p}><rect x="7" y="7" width="10" height="10" rx="1.5" /></Icon>
export const Folder = (p: IconProps) => <Icon {...p}><path d="M3.5 7.5h6l2-2h9v13h-17z" /></Icon>
export const Search = (p: IconProps) => <Icon {...p}><circle cx="10.5" cy="10.5" r="6" /><path d="M15 15l5 5" /></Icon>
export const Settings = (p: IconProps) => <Icon {...p}><circle cx="12" cy="12" r="3" /><path d="M12 3v2M12 19v2M3 12h2M19 12h2M5.6 5.6L7 7M17 17l1.4 1.4M18.4 5.6L17 7M7 17l-1.4 1.4" /></Icon>
export const Settings2 = (p: IconProps) => <Icon {...p}><path d="M4 6h8M16 6h4M4 12h3M11 12h9M4 18h10M18 18h2" /><circle cx="14" cy="6" r="2" /><circle cx="9" cy="12" r="2" /><circle cx="16" cy="18" r="2" /></Icon>
export const PanelLeft = (p: IconProps) => <Icon {...p}><rect x="3" y="4" width="18" height="16" rx="3" /><path d="M9 4v16" /></Icon>
export const SlidersHorizontal = (p: IconProps) => <Icon {...p}><path d="M4 7h5M13 7h7M4 12h10M18 12h2M4 17h2M10 17h10" /><circle cx="11" cy="7" r="2" /><circle cx="16" cy="12" r="2" /><circle cx="8" cy="17" r="2" /></Icon>

export const Menu = (p: IconProps) => <Icon {...p}><path d="M4 7h16M4 12h16M4 17h16" /></Icon>
export const MessageSquarePlus = (p: IconProps) => <Icon {...p}><path d="M5 5h14a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H10l-5 3v-3H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z" /><path d="M12 8v6M9 11h6" /></Icon>
export const Gauge = (p: IconProps) => <Icon {...p}><path d="M4.7 17a8 8 0 1 1 14.6 0" /><path d="M12 12l4-3M8 17h8" /></Icon>
export const Database = (p: IconProps) => <Icon {...p}><ellipse cx="12" cy="6" rx="7" ry="3" /><path d="M5 6v6c0 1.7 3.1 3 7 3s7-1.3 7-3V6M5 12v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6" /></Icon>
export const SquarePen = (p: IconProps) => <Icon {...p}><path d="M13 5H6a2 2 0 0 0-2 2v11a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2v-7" /><path d="M10 14l1-4 7-7 3 3-7 7-4 1z" /></Icon>
export const Flag = (p: IconProps) => <Icon {...p}><path d="M6 21V4M6 5c5-3 7 3 12 0v9c-5 3-7-3-12 0" /></Icon>
export const Lightbulb = (p: IconProps) => <Icon {...p}><path d="M9 18h6M10 21h4M8.5 14.5C7 13.4 6 11.7 6 9.8a6 6 0 1 1 12 0c0 1.9-1 3.6-2.5 4.7-.8.6-1.1 1.3-1.2 2H9.7c-.1-.7-.4-1.4-1.2-2z" /></Icon>
export const Layers3 = (p: IconProps) => <Icon {...p}><path d="M12 3l9 5-9 5-9-5 9-5zM3 12l9 5 9-5M3 16l9 5 9-5" /></Icon>
export const BarChart3 = (p: IconProps) => <Icon {...p}><path d="M4 20V10M10 20V4M16 20v-7M22 20H2" /></Icon>
export const Cpu = (p: IconProps) => <Icon {...p}><rect x="7" y="7" width="10" height="10" rx="2" /><path d="M9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3" /></Icon>
export const ShieldCheck = (p: IconProps) => <Icon {...p}><path d="M12 3l7 3v5c0 5-3 8-7 10-4-2-7-5-7-10V6l7-3z" /><path d="M8.5 12l2.2 2.2 4.8-5" /></Icon>
export const CheckCircle2 = (p: IconProps) => <Icon {...p}><circle cx="12" cy="12" r="9" /><path d="M8 12l2.5 2.5L16 9" /></Icon>
export const Terminal = (p: IconProps) => <Icon {...p}><rect x="3" y="4" width="18" height="16" rx="3" /><path d="M7 9l3 3-3 3M12 16h5" /></Icon>
export const Brain = (p: IconProps) => <Icon {...p}><path d="M9 5a3 3 0 0 0-5 2.2A3 3 0 0 0 5 13a3.5 3.5 0 0 0 4 5.5M15 5a3 3 0 0 1 5 2.2A3 3 0 0 1 19 13a3.5 3.5 0 0 1-4 5.5M9 5v14M15 5v14M9 9H6M15 9h3M9 14H6M15 14h3" /></Icon>
export const Bot = (p: IconProps) => <Icon {...p}><rect x="4" y="7" width="16" height="12" rx="3" /><path d="M12 3v4M9 12h.01M15 12h.01M8 16h8" /></Icon>
export const Sparkles = (p: IconProps) => <Icon {...p}><path d="M12 3l1.3 3.7L17 8l-3.7 1.3L12 13l-1.3-3.7L7 8l3.7-1.3L12 3zM5 14l.9 2.1L8 17l-2.1.9L5 20l-.9-2.1L2 17l2.1-.9L5 14zM19 14l.8 1.8 1.7.7-1.7.8L19 19l-.8-1.7-1.7-.8 1.7-.7L19 14z" /></Icon>
export const CircleAlert = (p: IconProps) => <Icon {...p}><circle cx="12" cy="12" r="9" /><path d="M12 7v6M12 17h.01" /></Icon>
export const Fingerprint = (p: IconProps) => <Icon {...p}><path d="M7 9a5 5 0 0 1 10 0c0 6-2 10-4 12M4 12c0-4 2-8 8-8s8 4 8 8M8 13c0 3-.5 5-2 7M12 8c2 0 3 1.5 3 4 0 3-.5 6-2 8M11 12c0 4-1 7-3 9" /></Icon>
export const KeyRound = (p: IconProps) => <Icon {...p}><circle cx="8" cy="12" r="4" /><path d="M12 12h9M18 12v3M15 12v2" /></Icon>

export const Pencil = (p: IconProps) => <Icon {...p}><path d="M4 20l4.2-1 10.6-10.6a2.1 2.1 0 0 0-3-3L5.2 16 4 20zM13.8 7.4l3 3" /></Icon>
export const Globe = (p: IconProps) => <Icon {...p}><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3c2.4 2.4 3.5 5.4 3.5 9S14.4 18.6 12 21M12 3C9.6 5.4 8.5 8.4 8.5 12s1.1 6.6 3.5 9" /></Icon>
export const Waveform = (p: IconProps) => <Icon {...p}><path d="M3 12h3l2-5 3.5 10 3-8 2 3H21" /></Icon>
export const Circle = (p: IconProps) => <Icon {...p}><circle cx="12" cy="12" r="4.5" /></Icon>
export const Stack3 = (p: IconProps) => <Icon {...p}><path d="M12 3l8 4-8 4-8-4 8-4zM4 11l8 4 8-4M4 15l8 4 8-4" /></Icon>
export const ExternalDrive = (p: IconProps) => <Icon {...p}><rect x="4" y="4" width="16" height="12" rx="2.5" /><path d="M7 12h.01M4 16h16M8 20h8" /></Icon>
export const RadioTower = (p: IconProps) => <Icon {...p}><path d="M12 9a3 3 0 0 1 0 6M12 9a3 3 0 0 0 0 6M8 6a8 8 0 0 0 0 12M16 6a8 8 0 0 1 0 12M12 15v6M9 21h6" /></Icon>

export const Copy = (p: IconProps) => <Icon {...p}><rect x="8" y="8" width="11" height="11" rx="2" /><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" /></Icon>
export const RotateCw = (p: IconProps) => <Icon {...p}><path d="M20 7v5h-5M19 12a7 7 0 1 0-2 5" /></Icon>
export const Photo = (p: IconProps) => <Icon {...p}><rect x="3" y="5" width="18" height="14" rx="2.5" /><circle cx="9" cy="10" r="1.7" /><path d="M4 17l5-4 3 2 3-4 5 6" /></Icon>
export const FileText = (p: IconProps) => <Icon {...p}><path d="M6 3h8l4 4v14H6zM14 3v5h4M9 12h6M9 16h6" /></Icon>
export const Info = (p: IconProps) => <Icon {...p}><circle cx="12" cy="12" r="9" /><path d="M12 11v6M12 7h.01" /></Icon>
export const TriangleAlert = (p: IconProps) => <Icon {...p}><path d="M10.3 4.2L2.7 17.4A2 2 0 0 0 4.4 20h15.2a2 2 0 0 0 1.7-2.6L13.7 4.2a2 2 0 0 0-3.4 0zM12 9v4M12 17h.01" /></Icon>
export const MacWindow = (p: IconProps) => <Icon {...p}><rect x="3" y="5" width="18" height="14" rx="2.5" /><path d="M3 9h18M7 7h.01M10 7h.01" /></Icon>
export const CornerDownLeft = (p: IconProps) => <Icon {...p}><path d="M20 5v6a4 4 0 0 1-4 4H5M9 11l-4 4 4 4" /></Icon>
export const WifiOff = (p: IconProps) => <Icon {...p}><path d="M2 8a16 16 0 0 1 4-2.4M10 4.2A16 16 0 0 1 22 8M5 12a11 11 0 0 1 5-2.5M14 9.5A11 11 0 0 1 19 12M8.5 15.5a5 5 0 0 1 7 0M12 20h.01M3 3l18 18" /></Icon>

export const ArrowUp = (p: IconProps) => (
  <Icon {...p}>
    <path d="M12 19V5" />
    <path d="M6.5 10.5L12 5l5.5 5.5" />
  </Icon>
)

export const Scope = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="6.5" />
    <circle cx="12" cy="12" r="2" />
    <path d="M12 2.5V6M12 18v3.5M2.5 12H6M18 12h3.5" />
  </Icon>
)

export const RectangleStack = (p: IconProps) => (
  <Icon {...p}>
    <rect x="5" y="5" width="14" height="10" rx="2" />
    <path d="M7 18h10M8.5 21h7" />
  </Icon>
)

export const Package = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 7.5L12 3l8 4.5v9L12 21l-8-4.5v-9z" />
    <path d="M4.5 7.5L12 12l7.5-4.5M12 12v9M8 5.2l8 4.6" />
  </Icon>
)

export const Wrench = (p: IconProps) => (
  <Icon {...p}>
    <path d="M14.5 6.5a4.5 4.5 0 0 0-6 5.8L3.8 17a2 2 0 0 0 2.8 2.8l4.7-4.7a4.5 4.5 0 0 0 5.8-6l-2.6 2.6-2.2-.6-.6-2.2 2.8-2.4z" />
  </Icon>
)

export const Plug = (p: IconProps) => (
  <Icon {...p}>
    <path d="M8 3v5M16 3v5M6 8h12v2a6 6 0 0 1-6 6 6 6 0 0 1-6-6V8zM12 16v5" />
  </Icon>
)

export const BookOpen = (p: IconProps) => (
  <Icon {...p}>
    <path d="M3.5 5.5A4.5 4.5 0 0 1 8 4c1.7 0 3.1.6 4 1.5V20c-.9-.9-2.3-1.5-4-1.5-1.8 0-3.3.6-4.5 1.5V5.5z" />
    <path d="M20.5 5.5A4.5 4.5 0 0 0 16 4c-1.7 0-3.1.6-4 1.5V20c.9-.9 2.3-1.5 4-1.5 1.8 0 3.3.6 4.5 1.5V5.5z" />
  </Icon>
)

export const Lock = (p: IconProps) => (
  <Icon {...p}>
    <rect x="5" y="10" width="14" height="10" rx="2.5" />
    <path d="M8.5 10V7.5a3.5 3.5 0 0 1 7 0V10M12 14v2" />
  </Icon>
)

export const Pause = (p: IconProps) => <Icon {...p}><path d="M9 6v12M15 6v12" /></Icon>
export const SignalBars = (p: IconProps) => <Icon {...p}><path d="M6 19v-3M12 19v-7M18 19V6" /></Icon>
export const SessionList = (p: IconProps) => <Icon {...p}><rect x="4" y="4" width="16" height="16" rx="3" /><path d="M8 9h8M8 12.5h8M8 16h5" /></Icon>
export const Bolt = (p: IconProps) => <Icon {...p}><path d="M13 3L5 13.5h6L10 21l8-10.5h-6L13 3z" /></Icon>
