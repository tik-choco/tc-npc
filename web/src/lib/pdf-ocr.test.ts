// Unit tests for pdf-ocr.ts's ocrPdfImages: the per-page OCR call is
// injected so this can run without a DOM (pdfjs-dist's rendering needs
// `document`/canvas, see renderPdfToImages, which has no test here — same as
// upstream tc-assistant2/src/pdfOcr.ts) or a real network call. What's worth
// locking down: page order is preserved, blank/whitespace-only pages are
// dropped rather than leaving empty paragraphs, and each page is told its
// own 1-based position plus the total count.
import { describe, expect, it, vi } from "vitest";
import { ocrPdfImages } from "./pdf-ocr";

describe("ocrPdfImages", () => {
  it("joins non-blank page results with a blank line, in page order", async () => {
    const ocrPage = vi.fn(async (imageUrl: string) => `text-for-${imageUrl}`);
    const result = await ocrPdfImages(["p1", "p2", "p3"], ocrPage);
    expect(result).toBe("text-for-p1\n\ntext-for-p2\n\ntext-for-p3");
  });

  it("passes the 1-based page number and total page count to each call", async () => {
    const calls: Array<[string, number, number]> = [];
    await ocrPdfImages(["a", "b"], async (imageUrl, pageNumber, totalPages) => {
      calls.push([imageUrl, pageNumber, totalPages]);
      return "x";
    });
    expect(calls).toEqual([
      ["a", 1, 2],
      ["b", 2, 2],
    ]);
  });

  it("drops pages whose OCR result is blank or whitespace-only", async () => {
    const ocrPage = async (imageUrl: string) => (imageUrl === "blank" ? "   \n  " : `text-${imageUrl}`);
    const result = await ocrPdfImages(["p1", "blank", "p2"], ocrPage);
    expect(result).toBe("text-p1\n\ntext-p2");
  });

  it("trims each page's text before joining", async () => {
    const result = await ocrPdfImages(["p1"], async () => "  hello world  \n");
    expect(result).toBe("hello world");
  });

  it("returns an empty string for an empty page list", async () => {
    const ocrPage = vi.fn();
    const result = await ocrPdfImages([], ocrPage);
    expect(result).toBe("");
    expect(ocrPage).not.toHaveBeenCalled();
  });

  it("returns an empty string when every page comes back blank", async () => {
    const result = await ocrPdfImages(["p1", "p2"], async () => "");
    expect(result).toBe("");
  });
});
