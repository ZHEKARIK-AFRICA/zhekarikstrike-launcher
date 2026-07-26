import { invoke } from '@tauri-apps/api/core';

import { initializeLanguage, t } from '../localization/i18n.js';
import { errorMessage } from './errors.js';
import { waitForE2eReady } from './e2e.js';
import { handleError, resetProgressUI, setupErrorModal, updateProgressBar } from './common.js';
import { listenUntilPageHide } from './event-listener.js';
import { navigateToPage } from './navigation.js';

setupErrorModal();

const startInstallButton = document.getElementById('start-install');
const cancelInstallButton = document.getElementById('cancel-install');
const installPathInput = document.getElementById('install-path');

function setInstalling(value) {
    if (startInstallButton) startInstallButton.disabled = value;
    if (cancelInstallButton) cancelInstallButton.disabled = !value;
}

document.addEventListener('DOMContentLoaded', async () => {
    try {
        await waitForE2eReady();
        await initializeLanguage();
        const savedPath = await invoke('get_game_path');
        if (savedPath && installPathInput && !installPathInput.value) installPathInput.value = savedPath;
        setInstalling(false);
        document.body.classList.add('fade-in');
    } catch (error) {
        handleError(null, `${t('stageMessages.error_loading_data')}: ${errorMessage(error)}`);
    }
});

startInstallButton?.addEventListener('click', async () => {
    const gamePath = installPathInput?.value?.trim();
    if (!gamePath) {
        handleError(null, t('errors.install_path_not_set'));
        return;
    }

    setInstalling(true);
    try {
        await invoke('install_game', { gamePath });
        resetProgressUI('progress-bar', 'install-status', 'progress-info', 'game-installed');
        document.body.classList.remove('fade-in');
        document.body.classList.add('fade-out');
        await navigateToPage('./public/index.html');
    } catch (error) {
        if (error?.code === 'canceled') {
            resetProgressUI('progress-bar', 'install-status', 'progress-info', 'cancel');
        } else {
            resetProgressUI('progress-bar', 'install-status', 'progress-info', 'error');
            handleError(null, `${t('errors.unknown_error')}: ${errorMessage(error)}`);
        }
    } finally {
        setInstalling(false);
    }
});

cancelInstallButton?.addEventListener('click', () => invoke('cancel_install'));

document.getElementById('choose-folder')?.addEventListener('click', async () => {
    const folderPath = await invoke('select_game_folder');
    if (folderPath && installPathInput) installPathInput.value = `${folderPath}\\ZHEKARIKSTRIKE`;
});

listenUntilPageHide('install-progress', ({ payload }) => {
    updateProgressBar(
        payload.progress, payload.stage, payload.timeRemainingSec,
        'progress-bar', 'install-status', 'progress-info'
    );
});
