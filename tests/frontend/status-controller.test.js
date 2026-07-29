// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest';

import { createStatusController } from '../../src/renderer/status-controller.js';

describe('status controller', () => {
    let translations;
    let controller;

    beforeEach(() => {
        document.body.innerHTML = `
            <main id="root">
                <p id="status"></p>
                <div id="progress"></div>
                <p id="info"></p>
            </main>
        `;
        translations = {
            checking: 'checking updates',
            failed: 'update check failed',
            installing: 'installing',
            ready: 'ready'
        };
        controller = createStatusController({
            root: document.getElementById('root'),
            statusElement: document.getElementById('status'),
            progressBar: document.getElementById('progress'),
            progressInfo: document.getElementById('info'),
            translate: (key) => translations[key] ?? key
        });
    });

    it('keeps a terminal error visible and rejects late progress from the settled operation', () => {
        controller.begin({ flow: 'play', step: 'updates', statusKey: 'checking' });
        controller.applyProgress({
            operationId: 'operation-1', stage: 'checking', progress: 35, timeRemainingSec: 8
        });
        controller.fail('failed');

        expect(document.getElementById('status').textContent).toBe('update check failed');
        expect(document.getElementById('progress').style.width).toBe('0%');
        expect(document.getElementById('info').textContent).toBe('');

        controller.begin({ flow: 'play', step: 'verify', statusKey: 'checking' });
        controller.applyProgress({
            operationId: 'operation-1', stage: 'install', progress: 99, timeRemainingSec: 1
        });

        expect(document.getElementById('progress').style.width).toBe('0%');
        expect(controller.getState().activeOperationId).toBeNull();
    });

    it('binds a frontend-created operation id before invoke can emit delayed progress', () => {
        controller.begin({
            flow: 'play', step: 'updates', statusKey: 'checking', operationId: 'new-operation'
        });
        controller.applyProgress({ operationId: 'old-operation', stage: 'checking', progress: 99 });

        expect(controller.getState().activeOperationId).toBe('new-operation');
        expect(document.getElementById('progress').style.width).toBe('0%');
    });

    it('treats checking and installation as independent zero-to-one-hundred stages', () => {
        controller.begin({ flow: 'verify', step: 'checking', statusKey: 'checking' });
        controller.applyProgress({ operationId: 'operation-2', stage: 'checking', progress: 100 });
        controller.applyProgress({ operationId: 'operation-2', stage: 'install', progress: 12 });

        expect(document.getElementById('progress').style.width).toBe('12%');
        expect(controller.getState()).toMatchObject({ kind: 'busy', progressStage: 'install' });

        controller.applyProgress({ operationId: 'operation-2', stage: 'complete', progress: 100 });
        expect(controller.getState().kind).toBe('busy');
    });

    it('rerenders the same semantic state after a language change', () => {
        controller.setIdle('ready');
        translations.ready = 'готово';
        window.dispatchEvent(new CustomEvent('language-changed', { detail: 'ru' }));

        expect(document.getElementById('status').textContent).toBe('готово');
        expect(controller.getState()).toMatchObject({ kind: 'idle', statusKey: 'ready' });
    });

    it('restores a coarse busy state from the Rust operation kind', () => {
        controller.restoreOperation('updating-game');

        expect(controller.getState()).toMatchObject({
            kind: 'busy', flow: 'restore', step: 'updating-game'
        });
        expect(document.getElementById('root').getAttribute('aria-busy')).toBe('true');
    });

    it('stops reacting to language events after dispose', () => {
        controller.setIdle('ready');
        controller.dispose();
        translations.ready = 'changed after disposal';
        window.dispatchEvent(new CustomEvent('language-changed'));

        expect(document.getElementById('status').textContent).toBe('ready');
    });
});
