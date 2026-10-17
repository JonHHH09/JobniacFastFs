/** Company and location as separate elements with an aria-hidden separator (no middle-dot string). */
export function MetaParts({
  parts,
  className = "",
}: {
  parts: readonly string[];
  className?: string;
}) {
  const shown = parts.filter((part) => part !== "");
  if (shown.length === 0) {
    return null;
  }
  return (
    <span
      className={`flex flex-wrap items-center gap-x-2 [overflow-wrap:anywhere] ${className}`.trim()}
    >
      {shown.map((part, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: static two-item list; parts may repeat
        <span key={index} className="flex items-center gap-x-2">
          {index > 0 ? <span aria-hidden className="h-3 w-px bg-border" /> : null}
          {part}
        </span>
      ))}
    </span>
  );
}
