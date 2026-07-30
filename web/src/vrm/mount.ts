// Imperative "put a VRM on this element" entry point.
//
// This module exists to be the code-splitting boundary: three.js and
// @pixiv/three-vrm together are far larger than the rest of the app, and
// most of the UI (settings, schedule, people, …) never shows a model at all.
// It is the ONLY module that statically imports them, and it is reached
// exclusively through a dynamic `import()` from components/VrmStage.tsx — so
// the 3D stack lands in its own chunk that a session which never opens an
// avatar layout never downloads.
//
// Keeping the three.js work here (rather than inside the component) also
// keeps it out of Preact's render cycle entirely: the frame loop reads live
// values through the `getSpeaking`/`getSpeakingLevel`/`getEmotion` callbacks
// the caller passes in, so a talking NPC never re-renders the tree.

import * as THREE from "three";
import type { VRM } from "@pixiv/three-vrm";
import { disposeVrm, loadVrmByFile } from "./loader";
import { createAvatarScene, type AvatarScene, type OrbitState, type VrmFraming } from "./stage";
import type { EmotionName } from "./animation";
import type { SpeakingLevelReading } from "./level";

/** Cap the device-pixel-ratio the canvas renders at: a 3x phone screen
 *  triples the fragment work for detail nobody can see on a talking head. */
const MAX_PIXEL_RATIO = 2;

export interface MountAvatarOptions {
  /** Model file name in the server's VRM folder. */
  file: string;
  framing: VrmFraming;
  /** Read every frame — true while the NPC's voice is audible. */
  getSpeaking: () => boolean;
  /** Read every frame — the latest loudness reading for the voice audible
   *  right now. Meaningless while getSpeaking() is false. */
  getSpeakingLevel: () => SpeakingLevelReading;
  /** Read every frame — the expression to wear. */
  getEmotion: () => EmotionName | null;
  /** Camera orbit to apply immediately after framing, e.g. restored from
   *  localStorage (see components/VrmStage.tsx). Omitted for the default
   *  front-on framing — a first-ever mount, or nothing saved yet for this
   *  model+framing pair. */
  initialOrbit?: OrbitState;
}

export interface MountedAvatar {
  /** Camera controls, driven by the component's pointer handlers. */
  orbit: AvatarScene["orbit"];
  /** Stop the frame loop and free the model, renderer and canvas. */
  dispose(): void;
}

/**
 * Load `file`, build a scene framed for `framing`, and render it into
 * `container` until the returned handle is disposed. Rejects if the model
 * can't be loaded or parsed; nothing is appended to `container` in that case.
 */
export async function mountAvatar(
  container: HTMLElement,
  options: MountAvatarOptions,
): Promise<MountedAvatar> {
  const vrm: VRM = await loadVrmByFile(options.file);

  const width = Math.max(1, container.clientWidth);
  const height = Math.max(1, container.clientHeight);
  const scene = createAvatarScene(vrm, width / height, options.framing);
  // Applied before the first render so a restored camera never flashes the
  // default framing for one frame and then jumps.
  if (options.initialOrbit) scene.orbit.restore(options.initialOrbit);

  // `alpha` plus a fully transparent clear colour is what lets the page (or,
  // in the `#/avatar` window, a chroma-key backdrop) show through around the
  // model instead of a black box.
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
  renderer.setClearColor(0x000000, 0);
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO));
  renderer.setSize(width, height, false);
  const canvas = renderer.domElement;
  canvas.className = "vrm-stage-canvas";
  container.appendChild(canvas);

  const observer = new ResizeObserver(() => {
    const w = container.clientWidth;
    const h = container.clientHeight;
    if (w === 0 || h === 0) return;
    scene.camera.aspect = w / h;
    scene.camera.updateProjectionMatrix();
    renderer.setSize(w, h, false);
  });
  observer.observe(container);

  let disposed = false;
  let frameId = 0;
  const clock = new THREE.Clock();

  const tick = () => {
    if (disposed) return;
    const delta = clock.getDelta();
    scene.animator.update(delta, options.getSpeaking(), options.getSpeakingLevel(), options.getEmotion());
    scene.vrm.update(delta);
    renderer.render(scene.scene, scene.camera);
    frameId = requestAnimationFrame(tick);
  };
  frameId = requestAnimationFrame(tick);

  return {
    orbit: scene.orbit,
    dispose() {
      if (disposed) return;
      disposed = true;
      cancelAnimationFrame(frameId);
      observer.disconnect();
      // Order matters: stop drawing, free the model's GPU resources, then
      // drop the context itself.
      disposeVrm(vrm);
      renderer.dispose();
      canvas.remove();
    },
  };
}
