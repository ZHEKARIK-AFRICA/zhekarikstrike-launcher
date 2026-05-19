// main.js

const axios = require('axios');
const fs = require('fs');
const { app, BrowserWindow, ipcMain, dialog, shell } = require('electron');
const path = require('path');
const { getGamePath, isGameExist } = require('./configManager'); // Import getGamePath
const { readRevIni, updateRevIni } = require('./revIniManager'); // Импортируем readRevIni
const { installGame, verifyFiles, updateGameVersion } = require('./installManager');
const { execFile } = require('child_process');
const sudo = require('sudo-prompt');
const { spawn } = require('child_process');
const { startRichPresence, stopRichPresence } = require('./RichPresence');
const { checkForUpdates, downloadAndReplaceLauncher } = require('./launcherManager'); // Менеджер обновлений
const { getLanguage, setLanguage } = require('./configManager');


let updateAbortController = null;
let verifyAbortController = null; // Для отмены проверки

let ProcessInProgress = false;

let mainWindow; // Declare mainWindow in the outer scope
let updateWindow;

let abortController = null; // Global variable to control cancellation


function createWindow() {
    mainWindow = new BrowserWindow({
        width: 892,
        height: 496,
        frame: false,
        webPreferences: {
            preload: path.join(__dirname, 'preload.js'),
            nodeIntegration: false,
            contextIsolation: true,
            enableRemoteModule: false,
            webSecurity: false
        }
    });

    mainWindow.setAspectRatio(1818 / 1123);

    // Сначала загружаем интро
    mainWindow.loadFile('./public/intro.html');

    // После проверки обновлений загружаем основную страницу
    checkForUpdates().then(() => {
        setTimeout(() => {
            if (mainWindow) {
                const nextPage = isGameExist() ? './public/index.html' : './public/install.html';
                mainWindow.webContents.send('start-fade-out', nextPage);
            }
        }, 3000);
    });
}

function createUpdateWindow() {
    updateWindow = new BrowserWindow({
        width: 892,
        height: 307,
        frame: false,
        webPreferences: {
            preload: path.join(__dirname, 'preload.js'),
            nodeIntegration: false,
            contextIsolation: true,
        }
    });

    updateWindow.setAspectRatio(1649 / 568);

    updateWindow.loadFile('./public/launcher_update.html');
}

ipcMain.on('minimize-window', () => {
    const window = BrowserWindow.getFocusedWindow();
    if (window) window.minimize();
});


ipcMain.on('close-window', () => {
    const window = BrowserWindow.getFocusedWindow();
    if (window) window.close();
});

ipcMain.on('navigate-to-page', (event, page) => {
    if (mainWindow) {
        mainWindow.loadFile(page);
    }
});


async function checkAndLaunch() {
    try {
        const isUpdateAvailable = await checkForUpdates();

        if (isUpdateAvailable) {
            console.log('Update available. Showing update window...');
            createUpdateWindow(); // Create update window before the update process starts

            // Start the update process
            await downloadAndReplaceLauncher(updateWindow);

            // Close update window after updating
            if (updateWindow) {
                updateWindow.close();
                updateWindow = null;
            }

            app.quit(); // Restart after the update
        } else {
            console.log('No updates available. Launching main window...');
            createWindow(); // Launch main window if no update is available
        }
    } catch (error) {
        console.error('Error during update check or download:', error);
        createWindow(); // If there's an error, still launch the main window
    }
}




function formatErrorMessage(error) {
    if (error.message.includes('ENOENT')) {
        return 'Необходимый файл отсутствует. Пожалуйста, попробуйте переустановить игру.';
    } else if (error.message.includes('Game path not set')) {
        return 'Не найден путь к игре. Пожалуйста, установите игру.';
    } else if (error.message.includes('Game executable not found')) {
        return 'Не найден файл игры. Проверьте установку.';
    } else if (error.message.includes('Launcher must be run as administrator')) {
        return 'Перезапустите лаунчер от имени администратора, иначе игра не запустится!';
    } else {
        return 'Произошла неизвестная ошибка. Пожалуйста, попробуйте снова.';
    }
}

ipcMain.on('open-external', (event, url) => {
    shell.openExternal(url).catch((err) => {
        console.error(`Failed to open URL: ${url}`, err);
    });
});



// Add the listener for 'load-main-page'
ipcMain.on('load-main-page', (event) => {
    if (mainWindow) {
        mainWindow.loadFile('./public/index.html');
    }
});



ipcMain.handle('launch-game', async (event) => {
    try {
        // Проверяем права администратора
        const isElevated = (await import('is-elevated')).default;
        const elevated = await isElevated();
        if (!elevated) {
            throw new Error('Launcher must be run as administrator to launch the game.');
        }

        const gamePath = getGamePath();
        if (!gamePath || !fs.existsSync(gamePath)) {
            throw new Error('Game path not set or does not exist');
        }

        const exePath = path.join(gamePath, 'RevLoader.exe');
        if (!fs.existsSync(exePath)) {
            throw new Error('Game executable not found');
        }

        // Запускаем Rich Presence
        startRichPresence();

        const child = spawn(exePath, [], { detached: true, stdio: 'ignore' });
        child.unref();

        child.on('close', (code) => {
            console.log(`Game process exited with code ${code}`);
            event.sender.send('game-closed', code);

            // Отключаем Rich Presence
            stopRichPresence();
        });

        return true;
    } catch (error) {
        console.error('Error launching game:', error);

        const userMessage = formatErrorMessage(error);
        event.sender.send('launch-error', userMessage);
    }
});


ipcMain.handle('get-game-data', async (event) => {
    try {
        const gamePath = getGamePath(); // Получаем путь к игре
        if (!gamePath || !fs.existsSync(gamePath)) {
            throw new Error('Game path not set or does not exist');
        }

        const { playerName, clanTag, launchParams } = readRevIni(gamePath);
        return { nickname: playerName, clanTag, launchParams, gamePath }; // Возвращаем также путь к игре
    } catch (error) {
        console.error('Error getting game data:', error);
        throw error;
    }
});

ipcMain.handle('update-rev-ini', async (event, { nickname, clanTag, launchParams }) => {
    try {
        const gamePath = getGamePath();
        if (!gamePath || !fs.existsSync(gamePath)) {
            throw new Error('Game path not set or does not exist');
        }
        updateRevIni(gamePath, nickname, clanTag, launchParams);
        return true;
    } catch (error) {
        console.error('Error updating rev.ini:', error);
        throw error;
    }
});

ipcMain.handle('select-folder', async () => {
    const result = await dialog.showOpenDialog({
        properties: ['openDirectory']
    });
    if (!result.canceled && result.filePaths.length > 0) {
        return result.filePaths[0];
    } else {
        return null;
    }
});

ipcMain.handle('get-current-state', async () => {
    return {
        ProcessInProgress,
    };
});

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



ipcMain.handle('cancel-install', (event) => {
    if (abortController) {
        abortController.abort();
        abortController = null;
        event.sender.send('install-canceled');
        return true;
    }
    return false;
});


// Обработчик для проверки файлов
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


// Обработчик для проверки файлов
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

app.whenReady().then(checkAndLaunch);

app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') {
        app.quit();
    }
});


// Отправляем текущий язык на запрос от рендерера
ipcMain.handle('get-language', async () => {
    return getLanguage();
});

// Обрабатываем запрос на смену языка
ipcMain.handle('set-language', async (event, lang) => {
    setLanguage(lang);
});

// Global error handlers to prevent the main process from crashing
process.on('uncaughtException', (error) => {
    console.error('Uncaught Exception:', error);
    // Notify renderer processes about the error
    if (mainWindow && mainWindow.webContents) {
        mainWindow.webContents.send('global-error', formatErrorMessage(error));
    }
    if (updateWindow && updateWindow.webContents) {
        updateWindow.webContents.send('global-error', formatErrorMessage(error));
    }
    // Optionally, display a native dialog
    dialog.showErrorBox('Unexpected Error', formatErrorMessage(error));
    // Depending on the severity, decide whether to quit the app
    // app.quit();
});

process.on('unhandledRejection', (reason, promise) => {
    console.error('Unhandled Rejection at:', promise, 'reason:', reason);
    // Notify renderer processes about the error
    if (mainWindow && mainWindow.webContents) {
        mainWindow.webContents.send('global-error', formatErrorMessage(reason));
    }
    if (updateWindow && updateWindow.webContents) {
        updateWindow.webContents.send('global-error', formatErrorMessage(reason));
    }
    // Optionally, display a native dialog
    dialog.showErrorBox('Unexpected Error', formatErrorMessage(reason));
    // Depending on the severity, decide whether to quit the app
    // app.quit();
});