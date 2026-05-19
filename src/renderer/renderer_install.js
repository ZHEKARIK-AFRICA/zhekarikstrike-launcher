import { handleError, setupErrorModal, updateProgressBar, resetProgressUI, updateStatus } from './common.js';

setupErrorModal();

const startInstallButton = document.getElementById('start-install');
const cancelInstallButton = document.getElementById('cancel-install');

if (startInstallButton) {
    startInstallButton.disabled = false;
}
if (cancelInstallButton) {
    cancelInstallButton.disabled = true;
}

document.addEventListener('DOMContentLoaded', async () => {
    try {
        document.body.classList.add('fade-in');
    } catch (error) {
        const translatedError = await window.electronAPI.t('stageMessages.error_loading_data');
        handleError(null, `${translatedError}: ${error.message}`);
    }
});

function fadeOutAndNavigate(page) {
    document.body.classList.remove('fade-in');
    document.body.classList.add('fade-out');

    document.body.addEventListener('transitionend', function handler() {
        document.body.removeEventListener('transitionend', handler);
        window.electronAPI.navigateToPage(page);
    });
}

if (startInstallButton && cancelInstallButton) {
    startInstallButton.addEventListener('click', async () => {
        const gamePath = document.getElementById('install-path').value;

        if (!gamePath || gamePath.trim() === '') {
            const translatedError = await window.electronAPI.t('errors.install_path_not_set');
            handleError(null, translatedError);
            return;
        }

        startInstallButton.disabled = true;
        cancelInstallButton.disabled = false;

        window.electronAPI.invoke('install-game', gamePath).then(() => {
            console.log('Installation started');
        }).catch(async (error) => {
            const translatedError = await window.electronAPI.t('errors.unknown_error');
            handleError(null, `${translatedError}: ${error.message}`);
            startInstallButton.disabled = false;
            cancelInstallButton.disabled = true;
        });
    });

    cancelInstallButton.addEventListener('click', async () => {
        const canceled = await window.electronAPI.invoke('cancel-install');
        if (canceled) {
            console.log('Installation canceled by user');
        } else {
            console.log('No active installation to cancel');
        }
    });
}

const chooseFolderButton = document.getElementById('choose-folder');
if (chooseFolderButton) {
    chooseFolderButton.addEventListener('click', async () => {
        let folderPath = await window.electronAPI.invoke('select-folder');
        if (folderPath) {
            folderPath = `${folderPath}\\ZHEKARIKSTRIKE`;
            document.getElementById('install-path').value = folderPath;
        }
    });
}

window.electronAPI.on('install-progress', async (event, { progress, stage, timeRemaining, errorMessage }) => {
    updateProgressBar(progress, stage, timeRemaining, 'progress-bar', 'install-status', 'progress-info');

    if (errorMessage) {
        const translatedError = await window.electronAPI.t('errors.unknown_error');
        handleError(event, `${translatedError}: ${errorMessage}`);
    }
});

window.electronAPI.on('install-complete', () => {
    console.log('Installation completed successfully');
    startInstallButton.disabled = false;
    cancelInstallButton.disabled = true;
    resetProgressUI('progress-bar', 'install-status', 'progress-info', 'game-installed');
    fadeOutAndNavigate('./public/index.html');
});

window.electronAPI.on('install-canceled', async () => {
    console.log('Installation canceled');
    startInstallButton.disabled = false;
    cancelInstallButton.disabled = true;
    resetProgressUI('progress-bar', 'install-status', 'progress-info', 'cancel');
});

window.electronAPI.on('install-error', async (event, errorMessage) => {
    const translatedError = await window.electronAPI.t('errors.unknown_error');
    console.error(`${translatedError}: ${errorMessage}`);
    startInstallButton.disabled = false;
    cancelInstallButton.disabled = true;
    resetProgressUI('progress-bar', 'install-status', 'progress-info', `error`);
    handleError(event, `${translatedError}: ${errorMessage}`);
});