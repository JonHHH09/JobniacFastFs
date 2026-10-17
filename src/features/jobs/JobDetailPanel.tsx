import { formatMessage } from "../../i18n/messages";
import type { JobDetailDto } from "../../ipc/bindings";
import { Badge } from "../../ui/badge";
import { MetaParts } from "./MetaParts";

/** The read-only view of the loaded job. */
export function JobDetailPanel({ job }: { job: JobDetailDto }) {
  return (
    <section
      data-testid="jobDetailPanel"
      aria-labelledby="job-detail-title"
      className="grid min-w-0 gap-3 p-5"
    >
      <div className="grid min-w-0 gap-1">
        <h2
          id="job-detail-title"
          data-testid="jobTitleLabel"
          className="text-xl leading-snug font-semibold [overflow-wrap:anywhere]"
        >
          {job.job.title}
        </h2>
        <MetaParts parts={[job.job.company, job.job.location]} className="text-muted-foreground" />
      </div>
      {job.skills.length > 0 ? (
        <ul
          aria-label={formatMessage("ui.job.detail.skills", { skills: job.skills.join(", ") })}
          className="flex min-w-0 flex-wrap gap-1.5"
        >
          {job.skills.map((skill) => (
            <li key={skill}>
              <Badge variant="outline" className="[overflow-wrap:anywhere]">
                {skill}
              </Badge>
            </li>
          ))}
        </ul>
      ) : null}
      <section
        aria-label={formatMessage("ui.job.detail.description")}
        data-testid="jobDescriptionLabel"
      >
        <p className="max-w-[68ch] leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere]">
          {job.job.description}
        </p>
      </section>
    </section>
  );
}
