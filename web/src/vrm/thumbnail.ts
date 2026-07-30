// Off-screen rendering of a single VRM bust thumbnail for the キャラ tab's
// model list.
//
// This is the ONLY module CharactersView.tsx reaches for the actual render,
// and only through a dynamic `import()` (see the queueing effect there) —
// exactly the split components/VrmStage.tsx uses for vrm/mount.ts, and for
// the same reason: three.js + @pixiv/three-vrm are far larger than the rest
// of the app, and the model *list* (file name, size, assign/delete buttons)
// must be usable the instant the tab opens, long before anyone's model has
// actually been decoded. Statically importing this file from the view would
// put the whole 3D stack back in the tab's own bundle, and from there into
// every other tab's initial load too.
//
// Reuses loader.ts/stage.ts rather than reimplementing loading or framing —
// the bust camera math in stage.ts is exactly what the live avatar stage
// uses, so a thumbnail and the live view agree on how a given model is
// framed.

import * as THREE from "three";
import { disposeVrm, loadVrmByFile } from "./loader";
import { createAvatarScene } from "./stage";
import { IDLE_SPEAKING_LEVEL } from "./level";

/** Square render target. The list only ever displays this at a few dozen CSS
 *  pixels (see .vrm-thumb in styles/characters.css), so this is sized for a
 *  crisp look on a hi-dpi screen without going anywhere near the resolution
 *  (or GPU fill-rate) a live, continuously-rendered stage would use. */
const THUMBNAIL_SIZE = 160;

/** Synthetic animation steps to run before capturing, at a fixed 60fps
 *  step. The arms-down standing pose itself is applied synchronously inside
 *  createVrmAnimator (see stage.ts's header comment on posed vs. T-pose
 *  bounds), so frame zero is already correct there — but auto-blink and the
 *  head gaze-follow each establish their own baseline only on their first
 *  update() call, and capturing on literally that first tick risks catching
 *  whatever that baseline-establishing transient looks like. A handful of
 *  fixed steps settles both before the still is taken, with no dependency on
 *  real wall-clock time (a hitch during rendering must not change what the
 *  thumbnail looks like). */
const SETTLE_STEPS = 5;
const SETTLE_DELTA_SECONDS = 1 / 60;

function canvasToBlob(canvas: HTMLCanvasElement): Promise<Blob> {
  return new Promise((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (blob) resolve(blob);
      else reject(new Error("canvas.toBlob returned null"));
    }, "image/png");
  });
}

/**
 * Load `file`, frame it as a bust shot, let the pose settle, capture one
 * frame as a PNG blob, and dispose the VRM and the renderer before
 * returning — the caller (CharactersView) only ever has one of these calls
 * in flight at a time, but this function disposes unconditionally either
 * way so a thrown error still can't leak a WebGL context or GPU buffers.
 * Rejects if the model can't be downloaded or parsed.
 */
export async function renderVrmThumbnail(file: string): Promise<Blob> {
  const vrm = await loadVrmByFile(file);
  try {
    const scene = createAvatarScene(vrm, 1, "bust");

    for (let i = 0; i < SETTLE_STEPS; i++) {
      scene.animator.update(SETTLE_DELTA_SECONDS, false, IDLE_SPEAKING_LEVEL, null);
      scene.vrm.update(SETTLE_DELTA_SECONDS);
    }

    // `alpha` + a transparent clear colour, same as the live stage
    // (mount.ts), so the thumbnail reads correctly against both the light
    // and dark theme's row background instead of carrying a baked-in black
    // square. `preserveDrawingBuffer` is what makes toBlob()/toDataURL() see
    // the frame just rendered — browsers are otherwise free to clear or
    // swap away the drawing buffer right after compositing. It costs a
    // (minor) swap-chain optimization, which is why the live stage in
    // mount.ts does NOT set it — but this renderer renders exactly one
    // frame and is disposed immediately after, so that cost is paid once
    // and never again.
    const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, preserveDrawingBuffer: true });
    try {
      renderer.setClearColor(0x000000, 0);
      renderer.setSize(THUMBNAIL_SIZE, THUMBNAIL_SIZE, false);
      scene.camera.aspect = 1;
      scene.camera.updateProjectionMatrix();
      renderer.render(scene.scene, scene.camera);
      return await canvasToBlob(renderer.domElement);
    } finally {
      renderer.dispose();
    }
  } finally {
    // Runs whether the try block above succeeded, threw, or returned —
    // a WebGL context or the model's GPU buffers must never outlive this
    // call, since the whole point of rendering thumbnails one-at-a-time is
    // to keep exactly one of these resources alive at any moment.
    disposeVrm(vrm);
  }
}
