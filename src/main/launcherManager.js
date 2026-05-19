// launcherManager.js

const axios = require('axios');
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');
const { app } = require('electron');
const logFilePath = path.join(app.getPath('userData'), 'launcher_log.txt');
const { getGamePath } = require('./configManager');

// Функция для записи логов в файл
function logToFile(message) {
    const timestamp = new Date().toISOString();
    console.log(message)
    fs.appendFileSync(logFilePath, `[${timestamp}] ${message}\n`, 'utf8');
}

const SERVER_URL = 'http://80.85.247.83:8000';
const UPDATE_ENDPOINT = '/version_number';
const DOWNLOAD_LAUNCHER_ENDPOINT = '/download_launcher';
const NEW_LAUNCHER_FILENAME = 'new_launcher.exe';

let currentVersion = app.getVersion();

/**
 * Проверка обновлений
 */
async function checkForUpdates() {
    logToFile('Checking for updates...');
    try {
        const response = await axios.get(`${SERVER_URL}${UPDATE_ENDPOINT}`);
        const latestVersion = response.data.launcher_version;
        logToFile(`Current version: ${currentVersion}, Latest version: ${latestVersion}`);
        return compareVersions(currentVersion, latestVersion) < 0;
    } catch (error) {
        logToFile(`Error checking for updates: ${error.message}`);
        return false;
    }
}

/**
 * Скачивание новой версии лаунчера с использованием progressCallback
 */
async function downloadAndReplaceLauncher(progressCallback) {
    const newLauncherPath = path.join(app.getPath('temp'), NEW_LAUNCHER_FILENAME);

    const portableExecutableFile = process.env.PORTABLE_EXECUTABLE_FILE;
    const execPath = process.execPath;
    let currentLauncherPath;
    let currentLauncherName;

    if (portableExecutableFile) {
        currentLauncherPath = portableExecutableFile;
        currentLauncherName = path.basename(currentLauncherPath);
    } else {
        currentLauncherPath = execPath;
        currentLauncherName = path.basename(currentLauncherPath);
    }

    logToFile(`Starting download of new launcher...`);
    logToFile(`Downloading to: ${newLauncherPath}`);

    try {
        const response = await axios({
            method: 'GET',
            url: `${SERVER_URL}${DOWNLOAD_LAUNCHER_ENDPOINT}`,
            responseType: 'stream',
        });

        const writer = fs.createWriteStream(newLauncherPath);
        response.data.pipe(writer);

        let totalDownloaded = 0;
        const totalSize = parseInt(response.headers['content-length'], 10);
        let startTime = Date.now();

        response.data.on('data', (chunk) => {
            totalDownloaded += chunk.length;
            const percentCompleted = (totalDownloaded / totalSize) * 100;

            const elapsedTime = (Date.now() - startTime) / 1000;
            const downloadSpeed = totalDownloaded / elapsedTime;

            const remainingBytes = totalSize - totalDownloaded;
            const estimatedTimeRemaining = downloadSpeed > 0 ? remainingBytes / downloadSpeed : 0;

            progressCallback(percentCompleted, 'download', estimatedTimeRemaining);
        });

        await new Promise((resolve, reject) => {
            writer.on('finish', () => {
                logToFile(`Download complete.`);
                resolve();
            });
            writer.on('error', (error) => {
                progressCallback(0, 'error', 0, error.message);
                reject(error);
            });
        });

        if (!fs.existsSync(newLauncherPath)) {
            throw new Error('New launcher not found after download.');
        }

        initiateUpdater(newLauncherPath, currentLauncherPath, currentLauncherName);
    } catch (error) {
        logToFile(`Error downloading new launcher: ${error.message}`);
        progressCallback(0, 'error', 0, error.message);
        throw error;
    }
}

/**
 * Инициация процесса обновления через временный batch-файл
 */
function initiateUpdater(newLauncherPath, currentLauncherPath, currentLauncherName) {
    logToFile('Initiating updater process.');

    const tempDir = app.getPath('temp');
    const updaterBatchPath = path.join(tempDir, 'updater.bat');

    const updaterCommands = `
echo REPLACING OLD LAUNCHER, AND LAUNCHING NEW VERSION
echo ОБНОВЛЯЮ ЛАУНЧЕР И ЗАПУСКАЮ НОВУЮ ВЕРСИЮ
@echo off
timeout /t 2 /nobreak
taskkill /IM "%~4" /F
del /F /Q "%~2"
move "%~3" "%~2"
start "" "%~2"
exit
`.trim();

    fs.writeFileSync(updaterBatchPath, updaterCommands, { encoding: 'utf8' });

    try {
        const child = spawn('cmd.exe', [
            '/c',
            updaterBatchPath,
            logFilePath,
            currentLauncherPath,
            newLauncherPath,
            currentLauncherName
        ], {
            detached: true,
            stdio: 'inherit',
            windowsHide: true  // Скрытое выполнение cmd.exe
        });
        child.unref();
    } catch (error) {
        logToFile(`Error executing updater batch file: ${error.message}`);
    }

    setTimeout(() => {
        app.exit(0);
    }, 50);
}

/**
 * Сравнение версий
 */
function compareVersions(v1, v2) {
    const v1Parts = v1.split('.').map(Number);
    const v2Parts = v2.split('.').map(Number);

    for (let i = 0; i < Math.max(v1Parts.length, v2Parts.length); i++) {
        const v1Part = v1Parts[i] || 0;
        const v2Part = v2Parts[i] || 0;

        if (v1Part > v2Part) return 1;
        if (v1Part < v2Part) return -1;
    }
    return 0;
}

const ws = require('windows-shortcuts');
let isMoveLauncherExecuted = false;  // Флаг для проверки выполнения функции


function moveLauncherAndCreateShortcut() {
    try {
        if (isMoveLauncherExecuted) {
            logToFile(isMoveLauncherExecuted);
            logToFile('moveLauncherAndCreateShortcut already executed. Skipping.');
            return;
        }

        logToFile('Starting moveLauncherAndCreateShortcut process.');

        const gamePath = getGamePath();
        const currentLauncherPath = process.env.PORTABLE_EXECUTABLE_FILE || process.execPath;
        const currentLauncherName = path.basename(currentLauncherPath);

        if (currentLauncherName.toLowerCase() === 'electron.exe') {
            logToFile('Detected electron.exe, skipping move operation.');
            return;
        }

        // Объявляем переменную newLauncherPath
        let newLauncherPath;

        if (currentLauncherName.toLowerCase() === 'csgo.exe') {
            newLauncherPath = path.join(gamePath, 'zhekarik_strike.exe');
        } else {
            newLauncherPath = path.join(gamePath, currentLauncherName);
        }

        const launcherShortcutName = 'ZHEKARIK STRIKE.lnk';

        if (!gamePath || !currentLauncherPath) {
            logToFile('Game path or current launcher path not found.');
            return;
        }

        if (currentLauncherPath === newLauncherPath) {
            return;
        }

        const tempDir = app.getPath('temp');
        const moveBatchFilePath = path.join(tempDir, 'move_launcher.bat');
        const logFilePath = path.join(tempDir, 'move_launcher.log');
        const moveCommands = `
    echo CREATING SHORTCUT ON DESKTOP FOR ZHEKARIK STRIKE LAUNCHER
    @echo off
    echo Start moving launcher >> "${logFilePath}"
    timeout /t 4 /nobreak
    move /Y "%~1" "%~2" >> "${logFilePath}" 2>&1
    if %ERRORLEVEL% EQU 0 (
        echo Launcher moved successfully. >> "${logFilePath}"
    ) else (
        echo Failed to move the launcher. >> "${logFilePath}"
    )
    pause
    exit
    `.trim();

        fs.writeFileSync(moveBatchFilePath, moveCommands, { encoding: 'utf8' });
        logToFile(`Move batch file created at: ${moveBatchFilePath}`);

        try {
            const startMenuPath = path.join(app.getPath('appData'), 'Microsoft', 'Windows', 'Start Menu', 'Programs');
            const desktopShortcutPath = path.join(app.getPath('desktop'), launcherShortcutName);  // Используем латиницу для теста
            const startMenuShortcutPath = path.join(startMenuPath, launcherShortcutName);  // Используем латиницу для теста

            // Выполним перемещение лаунчера через пакетный файл
            const child = spawn('cmd.exe', ['/c', moveBatchFilePath, currentLauncherPath, newLauncherPath], {
                detached: true,
                stdio: 'inherit',
            });
            child.unref();

            logToFile('Move batch file executed.');

            // Создаем ярлык на рабочем столе
            ws.create(desktopShortcutPath, {
                target: newLauncherPath,
                icon: newLauncherPath,
            }, function(err) {
                if (err) {
                    logToFile(`Failed to create desktop shortcut: ${err}`);
                } else {
                    logToFile('Desktop shortcut created successfully.');
                }
            });

            // Создаем ярлык в меню "Пуск"
            ws.create(startMenuShortcutPath, {
                target: newLauncherPath,
                icon: newLauncherPath,
            }, function(err) {
                if (err) {
                    logToFile(`Failed to create start menu shortcut: ${err}`);
                } else {
                    logToFile('Start menu shortcut created successfully.');
                }
            });

        } catch (error) {
            logToFile(`Error executing move batch file or creating shortcuts: ${error.message}`);
        }

        // Отметим, что функция была выполнена
        isMoveLauncherExecuted = true;

        // Exit the application after initiating the move
        setTimeout(() => {
            logToFile('Exiting the application.');
            app.exit(0);
        }, 50);
    } catch (error) {
        logToFile(error);
        app.exit(0);
    }
}

module.exports = { checkForUpdates, downloadAndReplaceLauncher, moveLauncherAndCreateShortcut };