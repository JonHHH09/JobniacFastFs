import type { ReactNode } from "react";
import { Card, CardContent, CardHeader } from "../../ui/card";

/**
 * One titled group of a section, a labelled region. The default is
 * a quiet panel (surface, 1px rule, 10px radius); `bare` drops the panel so the group sits inside
 * a surrounding workspace and is separated from its neighbours by the parent's hairlines.
 */
export function ProfileSection({
  id,
  title,
  testId,
  bare = false,
  actions,
  children,
}: {
  id: string;
  title: string;
  testId?: string;
  bare?: boolean;
  /** Buttons on the heading row (bare panels only). */
  actions?: ReactNode;
  children: ReactNode;
}) {
  const headingId = `${id}-heading`;
  if (bare) {
    return (
      <section aria-labelledby={headingId} data-testid={testId} className="grid min-w-0 gap-3 p-4">
        {actions ? (
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
            <h2 id={headingId} className="text-base leading-snug font-semibold">
              {title}
            </h2>
            {actions}
          </div>
        ) : (
          <h2 id={headingId} className="text-base leading-snug font-semibold">
            {title}
          </h2>
        )}
        {children}
      </section>
    );
  }
  return (
    <section aria-labelledby={headingId} data-testid={testId} className="min-w-0">
      <Card className="gap-4 py-4">
        <CardHeader>
          <h2 id={headingId} className="text-base leading-snug font-semibold">
            {title}
          </h2>
        </CardHeader>
        <CardContent className="grid min-w-0 gap-3">{children}</CardContent>
      </Card>
    </section>
  );
}
