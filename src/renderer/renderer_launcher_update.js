import { invoke } from '@tauri-apps/api/core';

import { initializeLanguage, t } from '../localization/i18n.js';
import { errorMessage } from './errors.js';
import { waitForE2eReady } from './e2e.js';
import { handleError, updateProgressBar, updateStatus } from './common.js';
import { listenUntilPageHide } from './event-listener.js';
import { navigateToPage } from './navigation.js';

function ensureContinueButton() {
    let button = document.getElementById('continue-without-update');
    if (button) return button;
    button = document.createElement('button');
    button.id = 'continue-without-update';
    button.className = 'modal-ok';
    button.type = 'button';
    button.textContent = 'продолжить без обновления';
    button.addEventListener('click', () => navigateToPage('./public/intro.html'));
    document.querySelector('footer')?.appendChild(button);
    return button;
}

document.addEventListener('DOMContentLoaded', async () => {
    try {
        await waitForE2eReady();
        await initializeLanguage();
        document.body.classList.add('fade-in');
        await invoke('download_launcher_update');
        updateStatus('complete', 'progress-status');
        await invoke('apply_launcher_update');
    } catch (error) {
        ensureContinueButton();
        handleError(null, `${t('stageMessages.error')}: ${errorMessage(error)}`);
    }
});

listenUntilPageHide('launcher-update-progress', ({ payload }) => {
    updateProgressBar(
        payload.progress, payload.stage, payload.timeRemainingSec,
        'progress-bar', 'progress-status', 'progress-info'
    );
});
