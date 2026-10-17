import { CircleAlert, CircleCheck, FileText } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { formatMessage, type MessageId } from "../../i18n/messages";
import type {
  ArtifactCapabilitiesDto,
  GenerationOutputDto,
  GenerationResultDto,
} from "../../ipc/bindings";
import { isCommandError, unknownFailure } from "../../ipc/commands";
import { humanizeEnum } from "../../lib/format";
import { Button } from "../../ui/button";
import { Card } from "../../ui/card";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { Label } from "../../ui/label";
import { Select } from "../../ui/select";
import { Separator } from "../../ui/separator";
import { Spinner } from "../../ui/spinner";
import { Textarea } from "../../ui/textarea";
import { ArtifactPreview } from "../artifacts/ArtifactPreview";
import { exportArtifact, fetchArtifactCapabilities } from "../artifacts/api";
import {
  exportDoneMessage,
  exportFailureMessage,
  messageText,
  NOT_ADMITTED,
  type UiMessage,
} from "../artifacts/artifact";
import { previewStoredDocument } from "../documents/api";
import { JobPicker, ProfilePicker, rememberDocument } from "../pickers";
import * as api from "./api";
import {
  buildGenerationCall,
  GENERATION_KINDS,
  type GenerationFields,
  type GenerationKind,
  generationFailureMessage,
  ID_MAX_LENGTH,
  kindMessageId,
  outputLabel,
  resultSummary,
  TEXT_MAX_LENGTH,
  visibleFields,
} from "./generation";

/** A catalogue message to show, or nothing. */
type Message = { id: MessageId } | null;

/** The text preview of one generated PDF. Shown only while its output is the selected one. */
type Preview = { documentId: string; text: string; truncated: boolean };

const EMPTY_FIELDS: GenerationFields = {
  fileName: "",
  title: "",
  body: "",
  profileId: "",
  jobId: "",
  resumeId: "",
  includeProjects: false,
};

/** The copy of the documents workspace that a failed preview shows (the preview reuses it). */
function previewFailureMessage(messageId: string): MessageId {
  return messageId === "documents.busy" ? "documents.busy" : "documents.error.generic";
}

/**
 * The PDF and resume generation screen. React holds the form and the
 * result list; Rust generates, stores and admits every identifier. A generated PDF is referenced by
 * its opaque document ID only, so no path reaches this screen. While the generation is running the
 * controls are locked, and one request runs at a time (the Rust guard too).
 */
/** The name shown for a generated PDF in the stored-document picker (no invented extension). */
function documentDisplayName(
  kind: GenerationKind,
  fileName: string,
  labelPrefix: GenerationOutputDto["labelPrefix"],
): string {
  const kindLabel = formatMessage(kindMessageId(kind));
  if (kind === "standalone") {
    const name = fileName.trim();
    if (name === "") {
      return kindLabel;
    }
    return name.toLowerCase().endsWith(".pdf") ? name : `${name}.pdf`;
  }
  return labelPrefix === "PDF"
    ? kindLabel
    : `${kindLabel} (${humanizeEnum("labelPrefix", labelPrefix)})`;
}

export function GenerationPage() {
  const [configured, setConfigured] = useState<boolean | null>(null);
  const [kind, setKind] = useState<GenerationKind>("standalone");
  const [fields, setFields] = useState<GenerationFields>(EMPTY_FIELDS);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<Message>(null);
  const [result, setResult] = useState<GenerationResultDto | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewMessage, setPreviewMessage] = useState<Message>(null);
  const [artifacts, setArtifacts] = useState<ArtifactCapabilitiesDto>(NOT_ADMITTED);
  const [viewing, setViewing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveMessage, setSaveMessage] = useState<UiMessage | null>(null);
  // A ref, not only the state, so that two clicks before the next render cannot both start a request.
  const inFlight = useRef(false);

  useEffect(() => {
    let active = true;
    api.fetchGenerationCapabilities().then(
      (capabilities) => {
        if (active) {
          setConfigured(capabilities.generation);
        }
      },
      () => {
        if (active) {
          setConfigured(false);
        }
      },
    );
    fetchArtifactCapabilities().then(
      (capabilities) => {
        if (active) {
          setArtifacts(capabilities ?? NOT_ADMITTED);
        }
      },
      () => {
        if (active) {
          setArtifacts(NOT_ADMITTED);
        }
      },
    );
    return () => {
      active = false;
    };
  }, []);

  const shown = visibleFields(kind);
  // Not admitted: every control is disabled. Busy: every control is locked.
  const unavailable = configured !== true;
  const locked = unavailable || busy;
  const selected = result?.outputs.find((output) => output.documentId === selectedId) ?? null;
  const previewShown = selected !== null && preview?.documentId === selected.documentId;

  function setField<K extends keyof GenerationFields>(name: K, value: GenerationFields[K]) {
    setFields((current) => ({ ...current, [name]: value }));
  }

  function chooseKind(next: string) {
    const found = GENERATION_KINDS.find((candidate) => candidate === next);
    if (found !== undefined) {
      setKind(found);
    }
  }

  async function generate() {
    if (inFlight.current || configured !== true) {
      return;
    }
    const call = buildGenerationCall(kind, fields);
    inFlight.current = true;
    setBusy(true);
    setMessage(null);
    setResult(null);
    setSelectedId(null);
    setPreview(null);
    setPreviewMessage(null);
    setViewing(false);
    setSaveMessage(null);
    try {
      const outcome =
        call.command === "standalone"
          ? await api.generateStandalone(call.request)
          : await api.generateResume(call.request);
      setResult(outcome);
      for (const output of outcome.outputs) {
        rememberDocument({
          id: output.documentId,
          displayName: documentDisplayName(kind, fields.fileName, output.labelPrefix),
          byteSize: output.byteSize,
        });
      }
      setSelectedId(outcome.outputs[0]?.documentId ?? null);
      // The cover letter needs the Germany resume ID: fill it once, when the field is still empty.
      if (outcome.resumeId !== null) {
        const resumeId = outcome.resumeId;
        setFields((current) => (current.resumeId === "" ? { ...current, resumeId } : current));
      }
      setMessage({ id: "generation.done" });
    } catch {
      setMessage({ id: generationFailureMessage() });
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }

  async function previewSelected() {
    if (selected === null || previewing || busy) {
      return;
    }
    const documentId = selected.documentId;
    setPreviewing(true);
    setPreviewMessage(null);
    try {
      const stored = await previewStoredDocument(documentId, false);
      setPreview({
        documentId: stored.document.id,
        text: stored.extraction.text,
        truncated: stored.extraction.truncated,
      });
    } catch (error) {
      const failure = isCommandError(error) ? error : unknownFailure;
      setPreview(null);
      setPreviewMessage({ id: previewFailureMessage(failure.messageId) });
    } finally {
      setPreviewing(false);
    }
  }

  /** Publishes the selected output to a file the user picks (Rust runs the dialog and the export). */
  async function saveSelected() {
    if (selected === null || saving || locked) {
      return;
    }
    setSaving(true);
    setSaveMessage(null);
    try {
      setSaveMessage(exportDoneMessage(await exportArtifact(selected.documentId)));
    } catch (error) {
      setSaveMessage(exportFailureMessage(error));
    } finally {
      setSaving(false);
    }
  }

  const summary = result === null ? "" : resultSummary(result);

  return (
    <section
      aria-labelledby="generation-heading"
      data-testid="generationPage"
      className="@container grid min-w-0 gap-4"
    >
      <Card className="gap-0 py-0">
        <fieldset className="grid min-w-0 gap-4 px-5 py-5">
          <legend className="sr-only">{formatMessage("generation.a11y.title")}</legend>
          <h2 id="generation-heading" className="text-base leading-snug font-semibold">
            {formatMessage("generation.title")}
          </h2>
          <p className="text-muted-foreground [overflow-wrap:anywhere]">
            {formatMessage("generation.intro")}
          </p>

          <Field label={formatMessage("generation.kind.label")} id="generationKind">
            {(control) => (
              <Select
                {...control}
                data-testid="generationKind"
                aria-label={formatMessage("generation.kind.label")}
                value={kind}
                disabled={unavailable || locked}
                onChange={(event) => chooseKind(event.target.value)}
              >
                {GENERATION_KINDS.map((option) => (
                  <option key={option} value={option}>
                    {formatMessage(kindMessageId(option))}
                  </option>
                ))}
              </Select>
            )}
          </Field>

          {shown.profileId || shown.jobId || shown.resumeId ? (
            <div className="grid min-w-0 gap-4 @[720px]:grid-cols-2">
              {shown.profileId ? (
                <Field label={formatMessage("generation.label.profileId")} id="generationProfileId">
                  {(control) => (
                    <ProfilePicker
                      {...control}
                      testId="generationProfileId"
                      label={formatMessage("generation.a11y.profileId")}
                      value={fields.profileId}
                      disabled={locked}
                      onChange={(id) => setField("profileId", id)}
                    />
                  )}
                </Field>
              ) : null}
              {shown.jobId ? (
                <Field label={formatMessage("generation.label.jobId")} id="generationJobId">
                  {(control) => (
                    <JobPicker
                      {...control}
                      testId="generationJobId"
                      label={formatMessage("generation.a11y.jobId")}
                      value={fields.jobId}
                      disabled={locked}
                      onChange={(id) => setField("jobId", id)}
                    />
                  )}
                </Field>
              ) : null}
              {shown.resumeId ? (
                <Field label={formatMessage("generation.label.resumeId")} id="generationResumeId">
                  {(control) => (
                    <Input
                      {...control}
                      data-testid="generationResumeId"
                      aria-label={formatMessage("generation.a11y.resumeId")}
                      maxLength={ID_MAX_LENGTH}
                      value={fields.resumeId}
                      disabled={locked}
                      onChange={(event) => setField("resumeId", event.target.value)}
                    />
                  )}
                </Field>
              ) : null}
            </div>
          ) : null}

          {shown.includeProjects ? (
            <div className="flex items-center gap-2">
              <input
                id="generationIncludeProjects"
                data-testid="generationIncludeProjects"
                type="checkbox"
                className="size-4 accent-primary"
                checked={fields.includeProjects}
                disabled={locked}
                onChange={(event) => setField("includeProjects", event.target.checked)}
              />
              <Label htmlFor="generationIncludeProjects">
                {formatMessage("generation.label.includeProjects")}
              </Label>
            </div>
          ) : null}

          {shown.fileName || shown.title ? (
            <div className="grid min-w-0 gap-4 @[720px]:grid-cols-2">
              {shown.fileName ? (
                <Field label={formatMessage("generation.label.fileName")} id="generationFileName">
                  {(control) => (
                    <Input
                      {...control}
                      data-testid="generationFileName"
                      aria-label={formatMessage("generation.a11y.fileName")}
                      maxLength={TEXT_MAX_LENGTH}
                      value={fields.fileName}
                      disabled={locked}
                      onChange={(event) => setField("fileName", event.target.value)}
                    />
                  )}
                </Field>
              ) : null}
              {shown.title ? (
                <Field label={formatMessage("generation.label.title")} id="generationTitle">
                  {(control) => (
                    <Input
                      {...control}
                      data-testid="generationTitle"
                      aria-label={formatMessage("generation.a11y.standaloneTitle")}
                      maxLength={TEXT_MAX_LENGTH}
                      value={fields.title}
                      disabled={locked}
                      onChange={(event) => setField("title", event.target.value)}
                    />
                  )}
                </Field>
              ) : null}
            </div>
          ) : null}

          {shown.body ? (
            <Field label={formatMessage("generation.label.body")} id="generationBody">
              {(control) => (
                <Textarea
                  {...control}
                  data-testid="generationBody"
                  aria-label={formatMessage("generation.a11y.body")}
                  rows={8}
                  value={fields.body}
                  disabled={locked}
                  onChange={(event) => setField("body", event.target.value)}
                />
              )}
            </Field>
          ) : null}
        </fieldset>

        <Separator />
        <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2 px-5 py-3">
          <Button
            type="button"
            data-testid="generatePdfButton"
            disabled={unavailable || locked}
            onClick={() => void generate()}
          >
            <FileText className="size-4" strokeWidth={1.75} aria-hidden />
            {formatMessage("generation.button.generate")}
          </Button>

          {busy ? (
            <p
              role="status"
              aria-label={formatMessage("generation.busy")}
              className="flex items-center gap-2 text-sm"
            >
              <Spinner />
              {formatMessage("generation.progress")}
            </p>
          ) : null}

          <div role="status" aria-live="polite" className="min-w-0 empty:hidden">
            {!busy && message !== null ? (
              <p
                data-testid="generationMessage"
                className={`flex min-w-0 items-start gap-2 text-sm [overflow-wrap:anywhere] ${message.id === "generation.done" ? "text-success" : "text-destructive"}`}
              >
                {message.id === "generation.done" ? (
                  <CircleCheck className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
                ) : (
                  <CircleAlert className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
                )}
                <span>{formatMessage(message.id)}</span>
              </p>
            ) : null}
          </div>
        </div>
      </Card>

      {result !== null && result.outputs.length > 0 ? (
        <Card className="gap-0 py-0">
          <div className="grid min-w-0 gap-4 px-5 py-5">
            {!busy && summary !== "" ? (
              <p data-testid="generationSummary" className="font-semibold [overflow-wrap:anywhere]">
                {summary}
              </p>
            ) : null}

            <Field
              label={formatMessage("generation.output.a11y")}
              id="generationOutputChoice"
              // One output needs no choice: the summary above already names it.
              className={result.outputs.length === 1 ? "sr-only" : undefined}
            >
              {(control) => (
                <Select
                  {...control}
                  data-testid="generationOutputChoice"
                  aria-label={formatMessage("generation.output.a11y")}
                  value={selectedId ?? ""}
                  disabled={locked}
                  onChange={(event) => {
                    setSelectedId(event.target.value);
                    setPreviewMessage(null);
                    setViewing(false);
                    setSaveMessage(null);
                  }}
                >
                  {result.outputs.map((output) => (
                    <option key={output.documentId} value={output.documentId}>
                      {outputLabel(output)}
                    </option>
                  ))}
                </Select>
              )}
            </Field>

            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="outline"
                data-testid="previewGeneratedPdfButton"
                disabled={unavailable || locked || previewing || selected === null}
                onClick={() => void previewSelected()}
              >
                {formatMessage("generation.preview.button")}
              </Button>
              {artifacts.artifacts ? (
                <Button
                  type="button"
                  data-testid="viewGeneratedPdfPagesButton"
                  disabled={locked || selected === null}
                  onClick={() => setViewing(true)}
                >
                  {formatMessage("generation.button.viewPages")}
                </Button>
              ) : null}
              {artifacts.artifacts && artifacts.export ? (
                <Button
                  type="button"
                  variant="outline"
                  data-testid="saveGeneratedPdfButton"
                  disabled={locked || saving || selected === null}
                  onClick={() => void saveSelected()}
                >
                  {formatMessage("artifacts.button.save")}
                </Button>
              ) : null}
            </div>

            {previewMessage !== null ? (
              <p className="text-sm [overflow-wrap:anywhere]">{formatMessage(previewMessage.id)}</p>
            ) : null}
            {saveMessage !== null ? (
              <p data-testid="generationExportMessage" className="text-sm [overflow-wrap:anywhere]">
                {messageText(saveMessage)}
              </p>
            ) : null}
          </div>

          {viewing && selected !== null && artifacts.artifacts ? (
            <>
              <Separator />
              <div className="min-w-0 px-5 py-5">
                <ArtifactPreview
                  key={selected.documentId}
                  documentId={selected.documentId}
                  available
                  save={false}
                  testIdPrefix="generatedArtifact"
                />
              </div>
            </>
          ) : null}
          {previewShown && preview !== null ? (
            <>
              <Separator />
              <div className="grid min-w-0 gap-1.5 px-5 py-5">
                <Label htmlFor="generatedPdfTextPreview">
                  {formatMessage("generation.preview.label")}
                </Label>
                <Textarea
                  id="generatedPdfTextPreview"
                  data-testid="generatedPdfTextPreview"
                  aria-label={formatMessage("generation.preview.a11y")}
                  readOnly
                  rows={8}
                  value={preview.text}
                />
              </div>
            </>
          ) : null}
        </Card>
      ) : null}
    </section>
  );
}
