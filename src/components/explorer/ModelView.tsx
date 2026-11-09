import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
import { OBJLoader } from "three/addons/loaders/OBJLoader.js";
import { PLYLoader } from "three/addons/loaders/PLYLoader.js";
import { STLLoader } from "three/addons/loaders/STLLoader.js";
import { ThreeMFLoader } from "three/addons/loaders/3MFLoader.js";
import type { FileEntry } from "@/lib/entries";
import { formatCount } from "@/lib/format";

/** The most of a model read: past this it would hold up the preview. */
const MAX_BYTES = 256 * 1024 * 1024;

/** A 3D model, lit and turning under the pointer (drag to turn, wheel to zoom, right-drag to move). */
export default function ModelView({ entry }: { entry: FileEntry }) {
    const host = useRef<HTMLDivElement>(null);
    const [status, setStatus] = useState<{ error?: string; triangles?: number; loading: boolean }>({ loading: true });

    useEffect(() => {
        const box = host.current;
        if (!box) return;
        let stopped = false;
        const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
        renderer.setPixelRatio(window.devicePixelRatio);
        box.appendChild(renderer.domElement);
        const scene = new THREE.Scene();
        const camera = new THREE.PerspectiveCamera(45, 1, 0.01, 1e7);
        scene.add(new THREE.HemisphereLight(0xffffff, 0x444455, 1.6));
        const sun = new THREE.DirectionalLight(0xffffff, 2.2);
        scene.add(sun);
        const controls = new OrbitControls(camera, renderer.domElement);
        controls.enableDamping = true;
        const size = () => {
            const { clientWidth: w, clientHeight: h } = box;
            renderer.setSize(w, h, false);
            renderer.domElement.style.width = "100%";
            renderer.domElement.style.height = "100%";
            camera.aspect = w / Math.max(1, h);
            camera.updateProjectionMatrix();
        };
        const observer = new ResizeObserver(size);
        observer.observe(box);
        size();
        let frame = 0;
        const tick = () => {
            controls.update();
            sun.position.copy(camera.position);
            renderer.render(scene, camera);
            frame = requestAnimationFrame(tick);
        };

        const material = new THREE.MeshStandardMaterial({ color: 0xb8c4d6, metalness: 0.1, roughness: 0.55, side: THREE.DoubleSide });
        const show = (object: THREE.Object3D) => {
            if (stopped) return;
            scene.add(object);
            const bounds = new THREE.Box3().setFromObject(object);
            const center = bounds.getCenter(new THREE.Vector3());
            const radius = Math.max(bounds.getSize(new THREE.Vector3()).length() / 2, 1e-6);
            object.position.sub(center);
            camera.position.set(radius * 1.6, radius * 1.1, radius * 1.8);
            camera.near = radius / 100;
            camera.far = radius * 100;
            camera.updateProjectionMatrix();
            controls.target.set(0, 0, 0);
            let triangles = 0;
            object.traverse((o) => {
                const mesh = o as THREE.Mesh;
                if (mesh.isMesh && mesh.geometry) triangles += (mesh.geometry.index ? mesh.geometry.index.count : (mesh.geometry.attributes.position?.count ?? 0)) / 3;
            });
            setStatus({ loading: false, triangles: Math.round(triangles) });
            tick();
        };
        const meshOf = (geometry: THREE.BufferGeometry) => {
            if (!geometry.attributes.normal) geometry.computeVertexNormals();
            const colored = !!geometry.attributes.color;
            return new THREE.Mesh(geometry, colored ? new THREE.MeshStandardMaterial({ vertexColors: true, side: THREE.DoubleSide }) : material);
        };

        (async () => {
            if (entry.size > MAX_BYTES) throw new Error("It's too big to draw here");
            const url = convertFileSrc(entry.path);
            const bytes = await (await fetch(url)).arrayBuffer();
            switch (entry.ext) {
                case "stl":
                    return show(meshOf(new STLLoader().parse(bytes)));
                case "ply":
                    return show(meshOf(new PLYLoader().parse(bytes)));
                case "obj": {
                    const group = new OBJLoader().parse(new TextDecoder().decode(bytes));
                    group.traverse((o) => {
                        const mesh = o as THREE.Mesh;
                        if (mesh.isMesh) mesh.material = material;
                    });
                    return show(group);
                }
                case "3mf":
                    return show(new ThreeMFLoader().parse(bytes));
                default: {
                    const gltf = await new GLTFLoader().parseAsync(bytes, url.slice(0, url.lastIndexOf("/") + 1));
                    return show(gltf.scene);
                }
            }
        })().catch((e) => !stopped && setStatus({ loading: false, error: String(e?.message ?? e) }));

        return () => {
            stopped = true;
            cancelAnimationFrame(frame);
            observer.disconnect();
            controls.dispose();
            renderer.dispose();
            renderer.domElement.remove();
        };
    }, [entry.path, entry.modified, entry.size, entry.ext]);

    return (
        <div className="relative h-full w-full">
            <div ref={host} className="absolute inset-0" />
            <div className="pointer-events-none absolute inset-x-0 bottom-2 text-center text-xs text-muted-foreground">
                {status.loading ? "Drawing it…" : status.error ? <span className="text-destructive">{status.error}</span> : `${formatCount(status.triangles ?? 0)} triangles · drag to turn, wheel to zoom`}
            </div>
        </div>
    );
}
