import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JobPicker } from "./JobPicker";
import { ProfilePicker } from "./ProfilePicker";
import { ReportPicker } from "./ReportPicker";
import { clearRecentDocuments, rememberDocument } from "./recentDocuments";
import { MAX_DOCUMENT_PAGES, StoredDocumentPicker } from "./StoredDocumentPicker";

const api = vi.hoisted(() => ({
  fetchProfilePage: vi.fn(),
  searchProfiles: vi.fn(),
  fetchProfile: vi.fn(),
  fetchJobPage: vi.fn(),
  searchJobs: vi.fn(),
  fetchJob: vi.fn(),
  loadDocumentMetadata: vi.fn(),
  fetchDocumentPage: vi.fn(),
  listReports: vi.fn(),
  getReport: vi.fn(),
}));
vi.mock("../profiles/api", () => ({
  fetchProfilePage: api.fetchProfilePage,
  searchProfiles: api.searchProfiles,
  fetchProfile: api.fetchProfile,
}));
vi.mock("../jobs/api", () => ({
  fetchJobPage: api.fetchJobPage,
  searchJobs: api.searchJobs,
  fetchJob: api.fetchJob,
}));
vi.mock("../documents/api", () => ({
  loadDocumentMetadata: api.loadDocumentMetadata,
  fetchDocumentPage: api.fetchDocumentPage,
}));
vi.mock("../matching/api", () => ({ listReports: api.listReports, getReport: api.getReport }));

const ADA = "00000000-0000-4000-8000-0000000000a1";
const GRACE = "00000000-0000-4000-8000-0000000000a2";
const JOB = "00000000-0000-4000-8000-0000000000b1";
const DOC = "00000000-0000-4000-8000-000600000001";

const ada = { id: ADA, fullName: "Ada Lovelace", email: "ada@example.com" };
const grace = { id: GRACE, fullName: "Grace Hopper", email: "grace@example.com" };

function Wrapper({ children }: { children: React.ReactNode }) {
  const [client] = useState(
    () => new QueryClient({ defaultOptions: { queries: { retry: false } } }),
  );
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

function Controlled({
  Picker,
  initial = "",
  onChange,
}: {
  Picker: typeof ProfilePicker;
  initial?: string;
  onChange?: (id: string) => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <>
      <Picker
        id="profileId"
        testId="profileIdInput"
        label="Profile"
        value={value}
        onChange={(id) => {
          setValue(id);
          onChange?.(id);
        }}
      />
      <span data-testid="value">{value}</span>
    </>
  );
}

function field(): HTMLElement {
  return screen.getByRole("combobox");
}

beforeEach(() => {
  api.fetchProfilePage.mockResolvedValue({ profiles: [ada, grace], nextCursor: null });
  api.searchProfiles.mockResolvedValue({
    profiles: [{ profile: grace, matchedFields: [], score: "1" }],
  });
  api.fetchProfile.mockResolvedValue({ profile: { ...ada, revision: "1" } });
  api.fetchJobPage.mockResolvedValue({
    jobs: [
      { id: JOB, title: "Platform Engineer", company: "Example Cloudworks", location: "Berlin" },
    ],
    nextCursor: null,
  });
  api.searchJobs.mockResolvedValue({ jobs: [] });
  api.fetchJob.mockResolvedValue({
    job: { id: JOB, title: "Platform Engineer", company: "Example Cloudworks", location: "Berlin" },
    skills: [],
  });
  api.loadDocumentMetadata.mockResolvedValue({
    id: DOC,
    displayName: "synthetic-cv.pdf",
    mediaType: "application/pdf",
    byteSize: 120 * 1024,
  });
  api.fetchDocumentPage.mockResolvedValue({ documents: [], nextCursor: null });
  api.listReports.mockResolvedValue({ reports: [], nextCursor: null });
});

afterEach(() => {
  vi.clearAllMocks();
  clearRecentDocuments();
});

describe("ProfilePicker", () => {
  it("keeps the id, test id and accessible name on the text input", () => {
    render(<Controlled Picker={ProfilePicker} />, { wrapper: Wrapper });
    const input = screen.getByRole("combobox", { name: "Profile" });
    expect(input).toHaveAttribute("id", "profileId");
    expect(input).toHaveAttribute("data-testid", "profileIdInput");
  });

  it("lists the first page on focus with name and email, and writes the chosen id", async () => {
    const user = userEvent.setup();
    render(<Controlled Picker={ProfilePicker} />, { wrapper: Wrapper });
    await user.click(field());
    expect(await screen.findByRole("option", { name: /Ada Lovelace/ })).toHaveTextContent(
      "ada@example.com",
    );
    expect(api.fetchProfilePage).toHaveBeenCalledWith(null);
    await user.click(screen.getByRole("option", { name: /Grace Hopper/ }));
    expect(screen.getByTestId("value")).toHaveTextContent(GRACE);
    expect(field()).toHaveValue("Grace Hopper");
  });

  it("waits 200 ms after the last key press and searches once", async () => {
    const user = userEvent.setup();
    render(<Controlled Picker={ProfilePicker} />, { wrapper: Wrapper });
    await user.click(field());
    await screen.findByRole("option", { name: /Ada Lovelace/ });
    await user.keyboard("gra");
    expect(api.searchProfiles).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toHaveTextContent("Searching…");
    await waitFor(() => expect(api.searchProfiles).toHaveBeenCalledTimes(1));
    expect(api.searchProfiles).toHaveBeenCalledWith("gra");
    const option = await screen.findByRole("option", { name: /Grace Hopper/ });
    expect(screen.queryByRole("option", { name: /Ada Lovelace/ })).not.toBeInTheDocument();
    expect(option).toHaveAttribute("aria-selected", "true");
  });

  it("accepts a pasted canonical UUID as the value without searching", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<Controlled Picker={ProfilePicker} onChange={onChange} />, { wrapper: Wrapper });
    await user.click(field());
    await user.paste(ADA);
    expect(onChange).toHaveBeenLastCalledWith(ADA);
    expect(screen.getByTestId("value")).toHaveTextContent(ADA);
    expect(field()).toHaveValue(ADA);
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(api.searchProfiles).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toHaveTextContent("Using the pasted ID.");
  });

  it("clears the value when the text is edited after a choice", async () => {
    const user = userEvent.setup();
    render(<Controlled Picker={ProfilePicker} />, { wrapper: Wrapper });
    await user.click(field());
    await user.click(await screen.findByRole("option", { name: /Ada Lovelace/ }));
    expect(screen.getByTestId("value")).toHaveTextContent(ADA);
    await user.type(field(), "x");
    expect(screen.getByTestId("value")).toBeEmptyDOMElement();
  });

  it("clears text and value with the clear button", async () => {
    const user = userEvent.setup();
    render(<Controlled Picker={ProfilePicker} />, { wrapper: Wrapper });
    await user.click(field());
    await user.click(await screen.findByRole("option", { name: /Ada Lovelace/ }));
    await user.click(screen.getByRole("button", { name: "Clear selection" }));
    expect(field()).toHaveValue("");
    expect(screen.getByTestId("value")).toBeEmptyDOMElement();
    expect(field()).toHaveFocus();
  });

  it("shows the name of an id that was set from outside", async () => {
    render(<Controlled Picker={ProfilePicker} initial={ADA} />, { wrapper: Wrapper });
    await waitFor(() => expect(field()).toHaveValue("Ada Lovelace"));
    expect(api.fetchProfile).toHaveBeenCalledWith(ADA);
  });

  it("shows an error row with a retry when the list cannot load", async () => {
    api.fetchProfilePage.mockRejectedValueOnce({ code: "internal_error", messageId: "" });
    const user = userEvent.setup();
    render(<Controlled Picker={ProfilePicker} />, { wrapper: Wrapper });
    await user.click(field());
    expect(await screen.findByText("Unable to load profiles. Try again.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("option", { name: /Ada Lovelace/ })).toBeInTheDocument();
  });

  it("shows an empty row for a search without results", async () => {
    api.searchProfiles.mockResolvedValue({ profiles: [] });
    const user = userEvent.setup();
    render(<Controlled Picker={ProfilePicker} />, { wrapper: Wrapper });
    await user.click(field());
    await user.keyboard("zzz");
    expect(await screen.findByText("No profiles found.")).toBeInTheDocument();
  });

  it("does not open or search while disabled", async () => {
    const user = userEvent.setup();
    render(
      <Wrapper>
        <ProfilePicker id="p" value="" onChange={() => undefined} disabled label="Profile" />
      </Wrapper>,
    );
    await user.click(field());
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(api.fetchProfilePage).not.toHaveBeenCalled();
  });
});

describe("JobPicker", () => {
  it("lists title with company and location and writes the chosen id", async () => {
    const user = userEvent.setup();
    render(<Controlled Picker={JobPicker} />, { wrapper: Wrapper });
    await user.click(field());
    const option = await screen.findByRole("option", { name: /Platform Engineer/ });
    expect(option).toHaveTextContent("Example Cloudworks, Berlin");
    await user.click(option);
    expect(screen.getByTestId("value")).toHaveTextContent(JOB);
    expect(field()).toHaveValue("Platform Engineer");
  });

  it("searches the saved jobs after the debounce", async () => {
    const user = userEvent.setup();
    render(<Controlled Picker={JobPicker} />, { wrapper: Wrapper });
    await user.click(field());
    await user.keyboard("cloud");
    await waitFor(() => expect(api.searchJobs).toHaveBeenCalledWith("cloud"));
    expect(api.searchJobs).toHaveBeenCalledTimes(1);
  });

  it("accepts a pasted UUID", async () => {
    const user = userEvent.setup();
    render(<Controlled Picker={JobPicker} />, { wrapper: Wrapper });
    await user.click(field());
    await user.paste(JOB);
    expect(screen.getByTestId("value")).toHaveTextContent(JOB);
  });
});

describe("StoredDocumentPicker", () => {
  it("offers the recently used documents, newest first, with size", async () => {
    rememberDocument({ id: DOC, displayName: "synthetic-cv.pdf", byteSize: 120 * 1024 });
    rememberDocument({
      id: "00000000-0000-4000-8000-000600000002",
      displayName: "letter.pdf",
      byteSize: 2048,
    });
    const user = userEvent.setup();
    render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
    await user.click(field());
    const options = await screen.findAllByRole("option");
    expect(options[0]).toHaveTextContent("letter.pdf");
    expect(options[1]).toHaveTextContent("synthetic-cv.pdf");
    expect(options[1]).toHaveTextContent("120 KB");
    await user.click(options[1] as HTMLElement);
    expect(screen.getByTestId("value")).toHaveTextContent(DOC);
    expect(field()).toHaveValue("synthetic-cv.pdf");
  });

  it("filters the recent documents by name without a command call", async () => {
    rememberDocument({ id: DOC, displayName: "synthetic-cv.pdf", byteSize: 1024 });
    rememberDocument({
      id: "00000000-0000-4000-8000-000600000002",
      displayName: "letter.pdf",
      byteSize: 2048,
    });
    const user = userEvent.setup();
    render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
    await user.click(field());
    await user.keyboard("letter");
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(1));
    expect(screen.getByRole("option")).toHaveTextContent("letter.pdf");
  });

  it("explains the paste fallback when nothing is stored yet", async () => {
    const user = userEvent.setup();
    render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
    await user.click(field());
    expect(
      await screen.findByText("No stored PDFs yet. Import a PDF first, or paste the ID of one."),
    ).toBeInTheDocument();
  });

  describe("with the stored-document listing", () => {
    const LETTER = "00000000-0000-4000-8000-000600000002";
    const listed = (id: string, displayName: string, pageCount: number | null) => ({
      id,
      displayName,
      mediaType: "application/pdf",
      byteSize: "122880",
      pageCount,
      createdAt: "2026-03-12T16:40:00Z",
    });

    it("lists stored PDFs with size, pages and date, and writes the chosen id", async () => {
      api.fetchDocumentPage.mockResolvedValue({
        documents: [listed(LETTER, "letter.pdf", 1), listed(DOC, "synthetic-cv.pdf", 2)],
        nextCursor: null,
      });
      const user = userEvent.setup();
      render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
      await user.click(field());
      const options = await screen.findAllByRole("option");
      expect(api.fetchDocumentPage).toHaveBeenCalledWith(null);
      expect(options).toHaveLength(2);
      expect(options[0]).toHaveTextContent("letter.pdf");
      expect(options[0]).toHaveTextContent("120 KB, 1 page,");
      expect(options[1]).toHaveTextContent("2 pages");
      await user.click(options[1] as HTMLElement);
      expect(screen.getByTestId("value")).toHaveTextContent(DOC);
      expect(field()).toHaveValue("synthetic-cv.pdf");
    });

    it("leaves out the page count of a document that was never extracted", async () => {
      api.fetchDocumentPage.mockResolvedValue({
        documents: [listed(LETTER, "letter.pdf", null)],
        nextCursor: null,
      });
      const user = userEvent.setup();
      render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
      await user.click(field());
      const option = await screen.findByRole("option");
      expect(option).toHaveTextContent("120 KB");
      expect(option).not.toHaveTextContent("page");
    });

    it("follows the cursor to later pages and shows each document once, recent first", async () => {
      api.fetchDocumentPage.mockImplementation(async (cursor: string | null) =>
        cursor === null
          ? { documents: [listed(LETTER, "letter.pdf", 1)], nextCursor: "next" }
          : { documents: [listed(DOC, "synthetic-cv.pdf", 2)], nextCursor: null },
      );
      rememberDocument({ id: DOC, displayName: "synthetic-cv.pdf", byteSize: 1024 });
      const user = userEvent.setup();
      render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
      await user.click(field());
      await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(2));
      expect(api.fetchDocumentPage).toHaveBeenNthCalledWith(1, null);
      expect(api.fetchDocumentPage).toHaveBeenNthCalledWith(2, "next");
      const options = screen.getAllByRole("option");
      expect(options[0]).toHaveTextContent("synthetic-cv.pdf");
      expect(options[1]).toHaveTextContent("letter.pdf");
    });

    it("stops after MAX_DOCUMENT_PAGES pages even when a cursor is always returned", async () => {
      api.fetchDocumentPage.mockResolvedValue({
        documents: [listed(LETTER, "letter.pdf", 1)],
        nextCursor: "next",
      });
      const user = userEvent.setup();
      render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
      await user.click(field());
      await screen.findAllByRole("option");
      await waitFor(() => expect(api.fetchDocumentPage).toHaveBeenCalledTimes(MAX_DOCUMENT_PAGES));
      expect(api.fetchDocumentPage).toHaveBeenNthCalledWith(1, null);
      expect(api.fetchDocumentPage).toHaveBeenNthCalledWith(MAX_DOCUMENT_PAGES, "next");
    });

    it("filters the loaded pages by file name after the debounce, with one listing call", async () => {
      api.fetchDocumentPage.mockResolvedValue({
        documents: [listed(LETTER, "letter.pdf", 1), listed(DOC, "synthetic-cv.pdf", 2)],
        nextCursor: null,
      });
      const user = userEvent.setup();
      render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
      await user.click(field());
      await screen.findAllByRole("option");
      await user.keyboard("LETT");
      await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(1));
      expect(screen.getByRole("option")).toHaveTextContent("letter.pdf");
      expect(api.fetchDocumentPage).toHaveBeenCalledTimes(1);
      await user.keyboard("zzz");
      expect(
        await screen.findByText("No stored PDF matches. Paste the ID of a stored PDF."),
      ).toBeInTheDocument();
    });

    it("shows an error row with a retry when the listing cannot load", async () => {
      api.fetchDocumentPage.mockRejectedValueOnce({
        code: "not_configured",
        messageId: "documents.intro.unavailable",
      });
      api.fetchDocumentPage.mockResolvedValue({
        documents: [listed(LETTER, "letter.pdf", 1)],
        nextCursor: null,
      });
      const user = userEvent.setup();
      render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
      await user.click(field());
      expect(
        await screen.findByText("Unable to load stored PDFs. Try again, or paste an ID."),
      ).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "Retry" }));
      expect(await screen.findByRole("option", { name: /letter\.pdf/ })).toBeInTheDocument();
    });
  });

  it("accepts a pasted id and remembers the document it finds", async () => {
    const user = userEvent.setup();
    render(<Controlled Picker={StoredDocumentPicker} />, { wrapper: Wrapper });
    await user.click(field());
    await user.paste(DOC);
    expect(screen.getByTestId("value")).toHaveTextContent(DOC);
    await waitFor(() => expect(api.loadDocumentMetadata).toHaveBeenCalledWith(DOC));
    // The raw text the user pasted stays as typed.
    expect(field()).toHaveValue(DOC);
    await user.clear(field());
    await user.click(field());
    expect(await screen.findByRole("option", { name: /synthetic-cv\.pdf/ })).toBeInTheDocument();
  });
});

describe("ReportPicker", () => {
  const REPORT = "00000000-0000-4000-8000-0000000000c1";
  const view = {
    report: {
      id: REPORT,
      profileId: ADA,
      jobId: JOB,
      createdAt: "2026-03-12T16:40:00Z",
      overallScore: 82,
    },
    stale: false,
  };

  it("lists reports by job title with profile, date and score, and writes the chosen id", async () => {
    api.listReports.mockResolvedValue({ reports: [view], nextCursor: null });
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<Controlled Picker={ReportPicker} onChange={onChange} />, { wrapper: Wrapper });
    await user.click(field());
    const option = await screen.findByRole("option", { name: /Platform Engineer/ });
    expect(api.listReports).toHaveBeenCalledWith(null, null, null);
    expect(option).toHaveTextContent("Ada Lovelace");
    expect(option).toHaveTextContent("score 82/100");
    expect(option).toHaveTextContent("2026");
    await user.click(option);
    expect(onChange).toHaveBeenLastCalledWith(REPORT);
    expect(field()).toHaveValue("Platform Engineer");
  });

  it("falls back to the generic job name, never the id, when a name cannot load", async () => {
    api.listReports.mockResolvedValue({ reports: [view], nextCursor: null });
    api.fetchJob.mockRejectedValue({ code: "not_found", messageId: "" });
    const user = userEvent.setup();
    render(<Controlled Picker={ReportPicker} />, { wrapper: Wrapper });
    await user.click(field());
    const option = await screen.findByRole("option");
    expect(option).not.toHaveTextContent(JOB);
    expect(option).toHaveTextContent("score 82/100");
  });

  it("filters by the typed text over the loaded reports", async () => {
    api.listReports.mockResolvedValue({ reports: [view], nextCursor: null });
    const user = userEvent.setup();
    render(<Controlled Picker={ReportPicker} />, { wrapper: Wrapper });
    await user.click(field());
    await screen.findByRole("option");
    await user.keyboard("zzz");
    expect(await screen.findByText(/No saved reports found/)).toBeInTheDocument();
    expect(api.listReports).toHaveBeenCalledTimes(1);
  });

  it("accepts a pasted UUID as it is and shows the name of an id set from outside", async () => {
    api.getReport.mockResolvedValue(view);
    const user = userEvent.setup();
    const { unmount } = render(<Controlled Picker={ReportPicker} />, { wrapper: Wrapper });
    await user.click(field());
    await user.paste(REPORT);
    expect(screen.getByTestId("value")).toHaveTextContent(REPORT);
    unmount();
    render(<Controlled Picker={ReportPicker} initial={REPORT} />, { wrapper: Wrapper });
    await waitFor(() => expect(field()).toHaveValue("Platform Engineer"));
    expect(api.getReport).toHaveBeenCalledWith(REPORT);
  });
});
