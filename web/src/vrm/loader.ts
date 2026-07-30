// Loading a VRM from the server's local model folder.
//
// The models live in `{data_dir}/vrm/` on the host and are fetched over
// `GET /api/vrm/file/:file` (see crates/npc-core/src/vrm.rs). Raw bytes are
// cached per file name so switching back and forth between avatar modes
// doesn't re-download tens of megabytes, but every mount parses its own VRM
// instance — a three.js object can only live in one scene at a time, so VRM
// instances are never shared.
//
// Ported from tc-town/src/vrm/loader.ts; the only real difference is where
// the bytes come from (an HTTP fetch here, IndexedDB there).

import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { VRMLoaderPlugin, VRMUtils, type VRM } from "@pixiv/three-vrm";

const loader = new GLTFLoader();
loader.register((parser) => new VRMLoaderPlugin(parser));

/** Downloaded bytes, keyed by model file name. */
const byteCache = new Map<string, ArrayBuffer>();

/** Parse a VRM from raw `.vrm` bytes (GLTFLoader + VRMLoaderPlugin). */
export async function loadVrmFromBytes(bytes: ArrayBuffer): Promise<VRM> {
  const gltf = await loader.parseAsync(bytes, "");
  const vrm = gltf.userData.vrm as VRM;
  VRMUtils.removeUnnecessaryVertices(gltf.scene);
  VRMUtils.combineSkeletons(gltf.scene);
  VRMUtils.combineMorphs(vrm);
  // VRM0.x models face +Z (away from our camera); this flips them 180deg to
  // face us. No-op for VRM1.0 models.
  VRMUtils.rotateVRM0(vrm);
  vrm.scene.traverse((object) => {
    object.frustumCulled = false;
  });
  return vrm;
}

/**
 * Fetch and parse the model named `file` from the server's model folder. The
 * returned VRM is a fresh instance owned by the caller (dispose it via
 * `disposeVrm` when the scene unmounts).
 */
export async function loadVrmByFile(file: string): Promise<VRM> {
  let bytes = byteCache.get(file);
  if (!bytes) {
    const res = await fetch(`/api/vrm/file/${encodeURIComponent(file)}`);
    if (!res.ok) throw new Error(`failed to load VRM ${file}: ${res.status}`);
    bytes = await res.arrayBuffer();
    byteCache.set(file, bytes);
  }
  // parseAsync detaches nothing, but a second parse of the same buffer must
  // see the original bytes — hand it a copy so the cache entry stays usable.
  return loadVrmFromBytes(bytes.slice(0));
}

/** Free all GPU/geometry resources held by a VRM instance. */
export function disposeVrm(vrm: VRM): void {
  VRMUtils.deepDispose(vrm.scene);
}

/** Drop a cached download, so a replaced model file is re-fetched. */
export function forgetVrmBytes(file: string): void {
  byteCache.delete(file);
}
