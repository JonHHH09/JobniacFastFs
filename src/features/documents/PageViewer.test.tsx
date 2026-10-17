import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { PageImageRefDto } from "../../ipc/bindings";
import { PageViewer } from "./PageViewer";

// Synthetic identifiers and names only.
const DOC_ID = "00000000-0000-4000-8000-0000000000d1";

const PAGE: PageImageRefDto = {
  documentId: DOC_ID,
  pageIndex: 0,
  pageCount: 2,
  width: 800,
  height: 600,
  revision: "1",
  url: "jobniac-page://localhost/handle-1/1",
};

/** One viewer with a page on screen; the prefix is omitted for the documents default. */
function viewer(testIdPrefix?: string) {
  return (
    <PageViewer
      testIdPrefix={testIdPrefix}
      page={PAGE}
      pending={null}
      disabled={false}
      zoom={1}
      imageFailed={false}
      onPage={vi.fn()}
      onZoom={vi.fn()}
      onImageError={vi.fn()}
    />
  );
}

describe("PageViewer", () => {
  it("keeps the documents test ids when no prefix is given", () => {
    const { getByTestId } = render(viewer());
    expect(getByTestId("document-viewer")).toBeInTheDocument();
    expect(getByTestId("document-viewer-image")).toBeInTheDocument();
    expect(getByTestId("document-pager-label")).toHaveTextContent("1 / 2");
    expect(getByTestId("document-zoom")).toBeInTheDocument();
  });

  it("sizes the image by zoom alone: auto height, no height cap and no height attribute", () => {
    const { getByTestId, rerender } = render(viewer());
    const image = getByTestId("document-viewer-image");
    expect(image).not.toHaveAttribute("height");
    expect(image.style.height).toBe("auto");
    expect(image.style.maxHeight).toBe("");
    expect(image.style.width).toBe("100%");
    rerender(
      <PageViewer
        page={PAGE}
        pending={null}
        disabled={false}
        zoom={0.5}
        imageFailed={false}
        onPage={vi.fn()}
        onZoom={vi.fn()}
        onImageError={vi.fn()}
      />,
    );
    expect(getByTestId("document-viewer-image").style.width).toBe("50%");
  });

  it("gives two viewers on one screen unique element ids and test ids", () => {
    const { container, getByTestId } = render(
      <>
        {viewer()}
        {viewer("artifact")}
      </>,
    );
    const ids = Array.from(container.querySelectorAll("[id]")).map((element) => element.id);
    expect(ids.length).toBeGreaterThan(0);
    expect(new Set(ids).size).toBe(ids.length);
    const testIds = Array.from(container.querySelectorAll("[data-testid]")).map((element) =>
      element.getAttribute("data-testid"),
    );
    expect(new Set(testIds).size).toBe(testIds.length);
    expect(getByTestId("artifact-viewer-image")).toBeInTheDocument();
    expect(getByTestId("artifact-pager-label")).toHaveTextContent("1 / 2");
  });
});
