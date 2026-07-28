import { invoke } from '@tauri-apps/api/core';

import { initializeLanguage, t } from '../localization/i18n.js';
import { errorMessage } from './errors.js';
import { waitForE2eReady } from './e2e.js';
import { handleError, setupErrorModal, updateProgressBar, updateStatus } from './common.js';
import { listenUntilPageHide } from './event-listener.js';

setupErrorModal();

document.addEventListener('DOMContentLoaded', async () => {
    try {
        await waitForE2eReady();
        await initializeLanguage();
        document.body.classList.add('fade-in');
        await invoke('download_launcher_update');
        updateStatus('complete', 'progress-status');
        await invoke('apply_launcher_update');
    } catch (error) {
        handleError(null, `${t('stageMessages.error')}: ${errorMessage(error)}`);
    }
});

listenUntilPageHide('launcher-update-progress', ({ payload }) => {
    updateProgressBar(
        payload.progress, payload.stage, payload.timeRemainingSec,
        'progress-bar', 'progress-status', 'progress-info'
    );
});
