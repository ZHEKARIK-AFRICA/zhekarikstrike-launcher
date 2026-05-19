// handlers/installHandlers.js

const { ipcMain } = require('electron');
const fs = require('fs');
const { getGamePath } = require('../configManager');
const { installGame, verifyFiles, updateGameVersion } = require('../installManager');
const axios = require('axios');

let ProcessInProgress = false;
let abortController = null;
let verifyAbortController = null;

function setupInstallHandlers(mainWindow) {
    // Получение текущего состояния (например, выполняется ли процесс)
    ipcMain.handle('get-current-state', async () => {
        return {
            ProcessInProgress,
        };
    });

    // Установка игры
    ipcMain.handle('install-game', async (event, gamePath) => {
        if (abortController) {
            event.sender.send('install-error', 'Installation is already in progress');
            return;
        }

        abortController = new AbortController();
        ProcessInProgress = true;
        const { signal } = abortController;

        const progressCallback = (progress, stage, timeRemaining, errorMessage) => {
            event.sender.send('install-progress', { progress, stage, timeRemaining, errorMessage });
        };

        try {
            await installGame(gamePath, progressCallback, signal);
            event.sender.send('install-complete');
        } catch (error) {
            console.error('Error during installation:', error);
            if (axios.isCancel(error) || signal.aborted) {
                event.sender.send('install-canceled');
            } else {
                event.sender.send('install-error', error.message || 'Unknown error occurred during installation');
            }
        } finally {
            ProcessInProgress = false;
            abortController = null;
            if (!event.sender.isDestroyed()) {
                event.sender.send('install-finalized');
            }
        }
    });

    // Отмена установки
    ipcMain.handle('cancel-install', (event) => {
        if (abortController) {
            abortController.abort();
            abortController = null;
            event.sender.send('install-canceled');
            return true;
        }
        return false;
    });

    // Проверка файлов игры
    ipcMain.handle('verify-files', async (event, checkAllFiles = true) => {
        if (verifyAbortController) {
            event.sender.send('verify-error', 'Verification is already in progress');
            return;
        }

        const gamePath = getGamePath();
        if (!gamePath || !fs.existsSync(gamePath)) {
            event.sender.send('verify-error', 'Game path not set or does not exist');
            return;
        }
        ProcessInProgress = true;

        verifyAbortController = new AbortController();
        const { signal } = verifyAbortController;

        const progressCallback = (progress, stage, timeRemaining, errorMessage) => {
            event.sender.send('verify-progress', { progress, stage, timeRemaining, errorMessage });
        };

        try {
            await verifyFiles(gamePath, progressCallback, signal, checkAllFiles);
            event.sender.send('verify-complete');
        } catch (error) {
            if (axios.isCancel(error) || error.name === 'AbortError' || error.message === 'Verification canceled by user') {
                event.sender.send('verify-canceled');
            } else {
                event.sender.send('verify-error', error.message);
            }
        } finally {
            verifyAbortController = null;
            ProcessInProgress = false;
        }
    });

    // Отмена проверки файлов
    ipcMain.handle('cancel-verify', (event) => {
        if (verifyAbortController) {
            ProcessInProgress = false;
            verifyAbortController.abort();
            verifyAbortController = null;
            event.sender.send('verify-canceled');
            return true;
        }
        return false;
    });

    // Обновление версии игры
    ipcMain.handle('update-game', async (event, checkAllFiles = true) => {
        if (verifyAbortController) {
            event.sender.send('update-error', 'Verification is already in progress');
            return;
        }

        ProcessInProgress = true;

        const gamePath = getGamePath();
        if (!gamePath || !fs.existsSync(gamePath)) {
            event.sender.send('update-error', 'Game path not set or does not exist');
            return;
        }

        verifyAbortController = new AbortController();
        const { signal } = verifyAbortController;

        const progressCallback = (progress, stage, timeRemaining, errorMessage) => {
            event.sender.send('verify-progress', { progress, stage, timeRemaining, errorMessage });
        };

        try {
            await updateGameVersion(progressCallback, signal);
            event.sender.send('verify-complete');
        } catch (error) {
            if (axios.isCancel(error) || error.name === 'AbortError' || error.message === 'Verification canceled by user') {
                event.sender.send('verify-canceled');
            } else {
                event.sender.send('verify-error', error.message);
            }
        } finally {
            verifyAbortController = null;
            ProcessInProgress = false;
        }
    });
}

module.exports = {
    setupInstallHandlers,
};