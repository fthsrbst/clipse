import { FileIcon, ImageIcon, LinkIcon, TextIcon } from "./icons";
import type { IconProps } from "./icons";
import { looksLikeLink } from "../lib/clip-content";
import type { Clip } from "../types/ipc";

/** Picks the small kind glyph for a clip row: images and files map
 * directly, a link-shaped text clip gets the link glyph, everything else
 * (plain text, HTML, RTF) reads as text.
 *
 * `Omit<IconProps, "clip">` because `IconProps` extends `SVGProps`, which
 * already declares the deprecated SVG `clip` presentation attribute — left
 * un-omitted, that attribute's type collides with our `clip: Clip` prop. */
export function KindIcon({ clip, ...rest }: { clip: Clip } & Omit<IconProps, "clip">) {
  if (clip.kind === "image") return <ImageIcon {...rest} />;
  if (clip.kind === "files") return <FileIcon {...rest} />;
  if (looksLikeLink(clip)) return <LinkIcon {...rest} />;
  return <TextIcon {...rest} />;
}
