// PDF → text for the チャット composer's file-drop entry point: render each
// page to a JPEG data URL with pdfjs-dist (renderPdfToImages), then run each
// page through the 視覚(vision) task's model via POST /api/llm/ocr
// (api.ts's ocrPdfPage — see that function's doc comment for why the route
// doesn't exist on the server yet) and join the non-empty pages together
// (ocrPdfImages). Ported from tc-assistant2/src/pdfOcr.ts: the rendering
// logic is unchanged, but the OCR call no longer goes straight from the
// browser to an OpenAI-compatible endpoint with a locally-held API key —
// tc-npc's provider/preset api keys are masked client-side (see
// config-types.ts), so the actual model call has to happen server-side; see
// lib/api.ts's ocrPdfPage for the proposed contract.
import pdfWorkerUrl from "pdfjs-dist/build/pdf.worker.mjs?url";
import { ocrPdfPage } from "./api";

/** Longest edge a rendered page is scaled to, capped at 2x the PDF's native
 *  size — matches tc-assistant2's constants.ts (PDF_RENDER_MAX_EDGE). Keeps
 *  page images (and therefore the vision model's request payload) a
 *  reasonable size regardless of how large the source PDF's page is. */
export const PDF_RENDER_MAX_EDGE = 1600;

/** Renders every page of `file` to a JPEG data URL, in page order. Requires a
 *  DOM (canvas + pdfjs' worker), so this only runs in the browser — there is
 *  no unit test for it here, matching tc-assistant2's own pdfOcr.ts. */
export async function renderPdfToImages(file: File): Promise<string[]> {
  const pdfjs = await import("pdfjs-dist");
  pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;
  const data = new Uint8Array(await file.arrayBuffer());
  const pdf = await pdfjs.getDocument({ data }).promise;
  const images: string[] = [];

  for (let pageNumber = 1; pageNumber <= pdf.numPages; pageNumber += 1) {
    const page = await pdf.getPage(pageNumber);
    const baseViewport = page.getViewport({ scale: 1 });
    const scale = Math.min(2, PDF_RENDER_MAX_EDGE / Math.max(baseViewport.width, baseViewport.height));
    const viewport = page.getViewport({ scale });
    const canvas = document.createElement("canvas");
    const context = canvas.getContext("2d");
    if (!context) {
      throw new Error("Canvas 2D context is unavailable");
    }

    canvas.width = Math.ceil(viewport.width);
    canvas.height = Math.ceil(viewport.height);
    await page.render({ canvas, canvasContext: context, viewport }).promise;
    images.push(canvas.toDataURL("image/jpeg", 0.9));
    canvas.width = 0;
    canvas.height = 0;
  }

  await pdf.cleanup();
  return images;
}

/**
 * Runs `ocrPage` over every rendered page image in order and joins the
 * non-blank results with a blank line between pages — a page the model
 * returned nothing useful for (a blank page, a rendering the model refused)
 * is silently dropped rather than leaving an empty paragraph in the output.
 *
 * `ocrPage` is injected (rather than this function calling api.ts directly)
 * so the join/trim/filter logic is unit-testable without a real network
 * call or a DOM — see pdf-ocr.test.ts. `ocrPdf` below is the real
 * end-to-end entry point ChatView uses.
 */
export async function ocrPdfImages(
  imageDataUrls: string[],
  ocrPage: (imageUrl: string, pageNumber: number, totalPages: number) => Promise<string>,
): Promise<string> {
  const pageTexts: string[] = [];

  for (const [index, imageUrl] of imageDataUrls.entries()) {
    const pageText = await ocrPage(imageUrl, index + 1, imageDataUrls.length);
    if (pageText.trim()) {
      pageTexts.push(pageText.trim());
    }
  }

  return pageTexts.join("\n\n");
}

/** End-to-end entry point for the composer's PDF drop: render every page,
 *  then OCR them one at a time through the server (see ocrPdfPage's doc
 *  comment on lib/api.ts for why the call is server-side rather than
 *  hitting an OpenAI-compatible endpoint directly from here). */
export async function ocrPdf(file: File): Promise<string> {
  const images = await renderPdfToImages(file);
  return ocrPdfImages(images, async (imageDataUrl, pageNumber, totalPages) => {
    const { text } = await ocrPdfPage({ imageDataUrl, pageNumber, totalPages });
    return text;
  });
}
