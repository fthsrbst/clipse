/**
 * Interface icons, inlined from `assets/brand/icons/` at the repo root.
 *
 * Every icon is `currentColor`, a `0 0 24 24` viewBox, and defaults to a
 * 1em box so it follows the surrounding text/button size unless a `size`
 * prop overrides it. Do not add an icon library — these are the complete,
 * hand-drawn set the brand ships.
 */

import type { SVGProps } from "react";

export interface IconProps extends SVGProps<SVGSVGElement> {
  size?: number | string;
}

function Svg({ size = "1em", children, ...rest }: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      width={size}
      height={size}
      aria-hidden="true"
      {...rest}
    >
      {children}
    </svg>
  );
}

export function CheckIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="m4 12.5 5 5 11-11" />
    </Svg>
  );
}

/** Opens the detail panel. Two corners rather than an eye: the panel does not
 * hide anything, it gives a clip the room a one-line row cannot. */
export function ExpandIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M14 4h6v6" />
      <path d="M10 20H4v-6" />
      <path d="M20 4l-7 7" />
      <path d="M4 20l7-7" />
    </Svg>
  );
}

export function ChevronDownIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="m5 9 7 7 7-7" />
    </Svg>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M5 5l14 14M19 5 5 19" />
    </Svg>
  );
}

export function CopyIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="8" y="8" width="12" height="12" rx="2" />
      <path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
    </Svg>
  );
}

export function DesktopIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="3" y="3" width="18" height="14" rx="2" />
      <path d="M12 17v4M8 21h8" />
    </Svg>
  );
}

export function DevicesIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="3" y="4" width="12.5" height="10" rx="1.5" />
      <path d="M3 17h13.5M9.25 14v3" />
      <rect x="18" y="7" width="3" height="10" rx="1" />
      <path d="M19 14.5h1" />
    </Svg>
  );
}

export function FileIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M6 3h8l4 4v14H6z" />
      <path d="M14 3v4h4" />
    </Svg>
  );
}

export function ImageIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <circle cx="8.5" cy="9" r="1.5" />
      <path d="m4 17 4.5-4.5 3.5 3 2.5-2.5 5.5 5" />
    </Svg>
  );
}

export function LaptopIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="3" y="4" width="18" height="13" rx="1.5" />
      <path d="M3 20h18M9 20h6" />
    </Svg>
  );
}

export function LinkIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="m9.5 14.5 5-5" />
      <path d="m7.5 16.5-1.3 1.3A3 3 0 0 1 2 13.6l2.8-2.8A3 3 0 0 1 9 10.7" />
      <path d="m16.5 7.5 1.3-1.3a3 3 0 0 1 4.2 4.2l-2.8 2.8a3 3 0 0 1-4.2.1" />
    </Svg>
  );
}

export function PauseIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M8 5v14M16 5v14" />
    </Svg>
  );
}

export function PinFilledIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M8 3h8l-1.5 5v3l2.5 2H7l2.5-2V8z" fill="currentColor" />
      <path d="M12 13v8" />
    </Svg>
  );
}

export function PinIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M8 3h8l-1.5 5v3l2.5 2H7l2.5-2V8z" />
      <path d="M12 13v8" />
    </Svg>
  );
}

export function PlayIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="m7 4 12 8-12 8z" />
    </Svg>
  );
}

export function SearchIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <circle cx="10.5" cy="10.5" r="6.5" />
      <path d="m15.5 15.5 5.5 5.5" />
    </Svg>
  );
}

export function SettingsIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <circle cx="12" cy="12" r="6.5" />
      <circle cx="12" cy="12" r="2.5" />
      <path d="M12 3v2.5M12 18.5V21M3 12h2.5M18.5 12H21M5.6 5.6l1.8 1.8M16.6 16.6l1.8 1.8M18.4 5.6l-1.8 1.8M7.4 16.6l-1.8 1.8" />
    </Svg>
  );
}

export function ShieldIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M12 2.5 19 5v6c0 4.5-2.8 8-7 10-4.2-2-7-5.5-7-10V5z" />
      <path d="M8.5 12s1.3-2.2 3.5-2.2 3.5 2.2 3.5 2.2-1.3 2.2-3.5 2.2S8.5 12 8.5 12zM8.5 8.5l7 7" />
    </Svg>
  );
}

export function TextIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M4 5h16M12 5v14M8 19h8" />
    </Svg>
  );
}

export function TrashIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M4 6h16M9 6V3h6v3M7 6l1 15h8l1-15M10 10v7M14 10v7" />
    </Svg>
  );
}

export function WifiOffIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M2.5 8.8a15 15 0 0 1 3.7-2.1M9.2 5.1a15 15 0 0 1 12.3 3.7M5.5 12.5a10 10 0 0 1 3.3-1.7M12.5 10.1a10 10 0 0 1 6 2.4M8.7 16.2a5 5 0 0 1 6.6 0M12 20h.01M3 3l18 18" />
    </Svg>
  );
}
