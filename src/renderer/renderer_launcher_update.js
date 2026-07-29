import { invoke } from '@tauri-apps/api/core';

import { initializeLanguage } from '../localization/i18n.js';
import { handleError, setupErrorModal } from './common.js';
import { waitForE2eReady } from './e2e.js';
import { listenUntilPageHide } from './event-listener.js';
import { createOperationId, createStatusController } from './status-controller.js';

setupErrorModal();

const status = createStatusController({
    root: document.querySelector('.launcher-container') || document.body,
    statusElement: document.getElementById('progress-status'),
    progressBar: document.getElementById('progress-bar'),
    progressInfo: document.getElementById('progress-info')
});
let disposed = false;
let pollTimer = null;

function updaterStatusKey(stage) {
    if (stage === 'checking') return 'status.checking_launcher_signature';
    if (stage === 'complete') return 'status.applying_launcher_update';
    return 'status.downloading_launcher_update';
}

document.addEventListener('DOMContentLoaded', async () => {
    try {
        await waitForE2eReady();
        await initializeLanguage();
        document.body.classList.add('fade-in');
        const current = await invoke('get_current_state');
        if (current?.operation === 'updating-launcher') {
            status.restoreOperation('updating-launcher');
            await waitForFinishedDownload();
        } else if (!current?.launcherUpdateReady) {
            const operationId = createOperationId();
            status.begin({
                flow: 'launcher-update',
                step: 'download',
                statusKey: 'status.downloading_launcher_update',
                operationId
            });
            await invoke('download_launcher_update', { operationId });
        }
        status.beginStep({
            step: 'apply',
            statusKey: 'status.applying_launcher_update'
        });
        await invoke('apply_launcher_update');
    } catch (error) {
        status.fail('status.launcher_update_failed');
        handleError(null, error, { contextKey: 'status.launcher_update_failed' });
    }
}, { once: true });

async function waitForFinishedDownload() {
    while (!disposed) {
        await new Promise((resolve) => {
            pollTimer = window.setTimeout(() => {
                pollTimer = null;
                resolve();
            }, 300);
        });
        if (disposed) return;
        const current = await invoke('get_current_state');
        if (current?.operation && current.operation !== 'idle') continue;
        if (!current?.launcherUpdateReady) {
            throw new Error('verified launcher update was not retained after download');
        }
        return;
    }
}

void listenUntilPageHide('launcher-update-progress', ({ payload }) => {
    status.applyProgress(payload, updaterStatusKey(payload.stage));
});

window.addEventListener('pagehide', () => {
    disposed = true;
    if (pollTimer != null) window.clearTimeout(pollTimer);
    status.dispose();
}, { once: true });
