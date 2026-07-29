import { describe, expect, it, vi } from 'vitest';

import {
    BASE_SPIN_DURATION_MS,
    advanceSpin,
    createAnimationDriver,
    createPointerCoalescer,
    loadSequentially
} from '../../src/renderer/3dmodel-runtime.js';

describe('3D model runtime', () => {
    it.each([30, 60, 144])('completes the base spin in 1.4 seconds at %i FPS', (fps) => {
        let progress = 0;
        const frameMs = 1000 / fps;
        for (let elapsed = 0; elapsed < BASE_SPIN_DURATION_MS; elapsed += frameMs) {
            progress = advanceSpin(progress, Math.min(frameMs, BASE_SPIN_DURATION_MS - elapsed), 1);
        }

        expect(BASE_SPIN_DURATION_MS).toBe(1400);
        expect(progress).toBeCloseTo(1, 6);
    });

    it('coalesces many mouse events into one update with the latest coordinates', () => {
        const handle = vi.fn();
        const coalescer = createPointerCoalescer({
            handle
        });

        coalescer.push({ clientX: 1, clientY: 2 });
        coalescer.push({ clientX: 3, clientY: 4 });
        coalescer.push({ clientX: 5, clientY: 6 });

        expect(handle).not.toHaveBeenCalled();
        coalescer.flush(10);
        expect(handle).toHaveBeenCalledOnce();
        expect(handle).toHaveBeenCalledWith({ clientX: 5, clientY: 6 }, 10);
    });

    it('loads the remaining models sequentially', async () => {
        let active = 0;
        let maximumActive = 0;
        const started = [];
        const loaded = await loadSequentially(['two', 'three', 'four'], async (url) => {
            active += 1;
            maximumActive = Math.max(maximumActive, active);
            started.push(url);
            await Promise.resolve();
            active -= 1;
            return `${url}-model`;
        });

        expect(started).toEqual(['two', 'three', 'four']);
        expect(loaded).toEqual(['two-model', 'three-model', 'four-model']);
        expect(maximumActive).toBe(1);
    });

    it('registers each sequentially loaded model before requesting the next one', async () => {
        const registered = [];
        await loadSequentially(['two', 'three'], async (url) => `${url}-model`, (model) => {
            registered.push(model);
        });

        expect(registered).toEqual(['two-model', 'three-model']);
    });

    it('stops scheduling frames while hidden and resumes without retaining the old timestamp', () => {
        const callbacks = new Map();
        let nextId = 0;
        const cancelFrame = vi.fn((id) => callbacks.delete(id));
        const onFrame = vi.fn();
        const driver = createAnimationDriver({
            requestFrame(callback) {
                nextId += 1;
                callbacks.set(nextId, callback);
                return nextId;
            },
            cancelFrame,
            onFrame
        });

        driver.start();
        callbacks.get(1)(100);
        expect(onFrame).toHaveBeenLastCalledWith(0, 100);

        driver.setVisible(false);
        expect(cancelFrame).toHaveBeenCalled();
        driver.setVisible(true);
        callbacks.get(3)(10_000);

        expect(onFrame).toHaveBeenLastCalledWith(0, 10_000);
    });
});
