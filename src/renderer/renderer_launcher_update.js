// renderer_launcher_update.js

import { updateProgressBar, handleError } from './common.js';


document.addEventListener('DOMContentLoaded', () => {
    console.log('Launcher update page loaded');
    document.body.classList.add('fade-in');

    window.electronAPI.on('update-progress', async (event, { progress, stage, timeRemaining, errorMessage }) => {
        updateProgressBar(progress, stage, timeRemaining, 'progress-bar', 'update-status', 'progress-info');

        if (errorMessage) {
            console.error(`Error during update: ${errorMessage}`);
            handleError(event, `${await window.electronAPI.t('stageMessages.error')}: ${errorMessage}`);
        }
    });
});