import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';

import {
    advanceSpin,
    createAnimationDriver,
    createPointerCoalescer,
    loadSequentially
} from './3dmodel-runtime.js';

const canvas = document.getElementById('modelCanvas');

if (canvas) {
    const renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.setSize(canvas.clientWidth, canvas.clientHeight, false);

    const camera = new THREE.PerspectiveCamera(
        40,
        canvas.clientWidth / canvas.clientHeight,
        0.01,
        10000
    );
    camera.position.z = 3;

    const scene = new THREE.Scene();
    const lights = [
        [0, 0.7, 1, 1.6],
        [-1.5, 0, 1, 1.6],
        [0.2, 0.2, 1, 0.1]
    ];
    for (const [x, y, z, intensity] of lights) {
        const light = new THREE.DirectionalLight(0x1e1e1e, intensity);
        light.position.set(x, y, z).normalize();
        scene.add(light);
    }

    const modelUrls = [
        new URL('../../public/assets/3dmodel/Trollface.glb', import.meta.url).href,
        new URL('../../public/assets/3dmodel/Trollface2.glb', import.meta.url).href,
        new URL('../../public/assets/3dmodel/Trollface3.glb', import.meta.url).href,
        new URL('../../public/assets/3dmodel/Trollface4.glb', import.meta.url).href,
        new URL('../../public/assets/3dmodel/Trollface5.glb', import.meta.url).href
    ];
    const loader = new GLTFLoader();
    const loadedModels = [];
    const raycaster = new THREE.Raycaster();
    const mouse = new THREE.Vector2();
    let model = null;
    let currentModelIndex = 0;
    let mouseRotationX = 0;
    let mouseRotationY = 0;
    let clickRotationY = 0;
    let spinProgress = 0;
    let speedMultiplier = 1;
    let spinning = false;
    let modelReplaced = false;
    let pointer = null;
    let preloadStarted = false;
    let disposed = false;

    function disposeObject(target) {
        target?.traverse((node) => {
            if (!node.isMesh) return;
            node.geometry?.dispose();
            const materials = Array.isArray(node.material) ? node.material : [node.material];
            materials.forEach((material) => material?.dispose());
        });
    }

    function configureModel(target) {
        target.position.y = 0.15;
        target.scale.set(10, 10, 10);
        target.traverse((node) => {
            if (!node.isMesh) return;
            node.frustumCulled = false;
            node.material.metalness = 1;
            node.material.roughness = 0.4;
            node.material.needsUpdate = true;
        });
        return target;
    }

    function loadModel(url) {
        return new Promise((resolve, reject) => {
            loader.load(url, (gltf) => {
                const target = gltf.scene;
                if (disposed) {
                    disposeObject(target);
                    resolve(null);
                    return;
                }
                resolve(configureModel(target));
            }, undefined, reject);
        });
    }

    function replaceModel() {
        if (loadedModels.length < 2) return;
        let nextIndex = currentModelIndex;
        while (nextIndex === currentModelIndex) {
            nextIndex = Math.floor(Math.random() * loadedModels.length);
        }
        currentModelIndex = nextIndex;
        scene.remove(model);
        model = configureModel(loadedModels[nextIndex].clone());
        scene.add(model);
    }

    const driver = createAnimationDriver({
        requestFrame: (callback) => window.requestAnimationFrame(callback),
        cancelFrame: (id) => window.cancelAnimationFrame(id),
        onFrame(elapsedMs, timestamp) {
            if (disposed) return;
            pointer?.flush(timestamp);
            if (model && spinning) {
                spinProgress = advanceSpin(spinProgress, elapsedMs, speedMultiplier);
                const eased = 0.5 * (1 - Math.cos(Math.PI * spinProgress));
                clickRotationY = Math.PI * 2 * eased;
                if (spinProgress >= 0.75 && !modelReplaced) {
                    replaceModel();
                    modelReplaced = true;
                }
                if (spinProgress >= 1) {
                    spinning = false;
                    spinProgress = 0;
                    clickRotationY = 0;
                    speedMultiplier = 1;
                    modelReplaced = false;
                }
            }
            if (model) {
                model.rotation.y = clickRotationY + mouseRotationY;
                model.rotation.x = mouseRotationX;
            }
            renderer.render(scene, camera);
            if (model && !preloadStarted) {
                preloadStarted = true;
                void loadSequentially(modelUrls.slice(1), loadModel, (loaded) => {
                    if (disposed || !loaded) {
                        disposeObject(loaded);
                        return;
                    }
                    loadedModels.push(loaded);
                }).catch((error) => {
                    if (!disposed) console.error('Failed to preload 3D model:', error);
                });
            }
        }
    });

    pointer = createPointerCoalescer({
        handle(event) {
            if (!model) return;
            const rect = canvas.getBoundingClientRect();
            const deltaX = event.clientX - (rect.left + rect.width / 2);
            const deltaY = event.clientY - (rect.top + rect.height / 2);
            mouseRotationY = deltaX * 0.001;
            mouseRotationX = deltaY * 0.001;

            const inside = event.clientX >= rect.left && event.clientX <= rect.right
                && event.clientY >= rect.top && event.clientY <= rect.bottom;
            if (!inside || rect.width === 0 || rect.height === 0) {
                canvas.style.cursor = 'default';
                return;
            }
            mouse.x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
            mouse.y = -((event.clientY - rect.top) / rect.height) * 2 + 1;
            raycaster.setFromCamera(mouse, camera);
            canvas.style.cursor = raycaster.intersectObject(model, true).length > 0
                ? 'pointer'
                : 'default';
        }
    });

    function onMouseMove(event) {
        pointer.push(event);
    }

    function onCanvasClick() {
        if (!model) return;
        if (!spinning) {
            spinning = true;
            spinProgress = 0;
            speedMultiplier = 1;
            modelReplaced = false;
        } else {
            speedMultiplier = Math.min(4, speedMultiplier + 0.4);
        }
    }

    function onResize() {
        const width = canvas.clientWidth;
        const height = canvas.clientHeight;
        if (!width || !height) return;
        renderer.setSize(width, height, false);
        camera.aspect = width / height;
        camera.updateProjectionMatrix();
    }

    function onVisibilityChange() {
        driver.setVisible(!document.hidden);
    }

    function disposeScene() {
        if (disposed) return;
        disposed = true;
        pointer.cancel();
        driver.stop();
        window.removeEventListener('mousemove', onMouseMove);
        window.removeEventListener('resize', onResize);
        canvas.removeEventListener('click', onCanvasClick);
        document.removeEventListener('visibilitychange', onVisibilityChange);
        for (const loaded of loadedModels) {
            disposeObject(loaded);
        }
        loadedModels.length = 0;
        renderer.dispose();
    }

    window.addEventListener('mousemove', onMouseMove, { passive: true });
    window.addEventListener('resize', onResize);
    canvas.addEventListener('click', onCanvasClick);
    document.addEventListener('visibilitychange', onVisibilityChange);
    window.addEventListener('pagehide', disposeScene, { once: true });

    loadModel(modelUrls[0]).then((initialModel) => {
        if (disposed || !initialModel) {
            disposeObject(initialModel);
            return;
        }
        model = initialModel;
        loadedModels.push(initialModel);
        scene.add(model);
        driver.start();
    }).catch((error) => {
        console.error('An error occurred loading the model:', error);
    });
}
