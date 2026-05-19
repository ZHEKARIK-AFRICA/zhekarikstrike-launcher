// windowManager.js

const { BrowserWindow, app, Menu } = require('electron');
const path = require('path');
const { isGameExist, getLanguage } = require('./configManager');
const { checkForUpdates, downloadAndReplaceLauncher, moveLauncherAndCreateShortcut } = require('./launcherManager');
const { setMainWindow, setUpdateWindow } = require('./errorHandler');
const { setupIpcHandlers } = require('./ipcHandlers');
const { getGameProcess } = require('./handlers/gameHandlers');
const { stopRichPresence } = require('./RichPresence');
const { deleteCopiedFiles, copiedFilesGameFilesPure, copiedFilesGameFiles } = require('./fileManager'); // Corrected import

let mainWindow;
let updateWindow;

async function createAppropriateWindow() {
    // First, check for updates
    const hasUpdate = await checkForUpdates();

    if (hasUpdate) {
        // If an update is available, create the update window
        updateWindow = new BrowserWindow({
            width: 788,
            height: 272,
            frame: false,
            webPreferences: {
                preload: path.join(__dirname, 'preload.js'),
                nodeIntegration: false,
                contextIsolation: true,
                backgroundThrottling: false,
                defaultFontFamily: 'sansSerif',
                disableHtmlFullscreenWindowResize: true,
                spellcheck: false,
            },
        });

        updateWindow.setBackgroundColor('#1E1E1E');

        updateWindow.setAspectRatio(1649 / 568);

        // Get the current language
        const currentLanguage = getLanguage();

        // Send the language to the renderer
        updateWindow.webContents.on('did-finish-load', () => {
            updateWindow.webContents.send('set-language', currentLanguage);
        });

        updateWindow.loadFile('./public/launcher_update.html');
        setupIpcHandlers(updateWindow);

        // Set the update window in error handler for error reporting
        setUpdateWindow(updateWindow);

        // Start the download and replace process
        downloadAndReplaceLauncher((progress, stage, timeRemaining, errorMessage) => {
            // Send progress updates to the update window via IPC
            if (updateWindow && updateWindow.webContents) {
                updateWindow.webContents.send('update-progress', {
                    progress,
                    stage,
                    timeRemaining,
                    errorMessage
                });
            }
        }).catch(error => {
            // Handle errors if necessary
            console.error('Error during launcher update:', error);
            // You might want to show an error message to the user here
        });

    } else {
        // If no updates are available, create the main window
        mainWindow = new BrowserWindow({
            width: 892,
            height: 496,
            frame: false,
            webPreferences: {
                preload: path.join(__dirname, 'preload.js'),
                nodeIntegration: false,
                contextIsolation: true,
                enableRemoteModule: false,
                webSecurity: false,
                backgroundThrottling: false,
                defaultFontFamily: 'sansSerif',
                disableHtmlFullscreenWindowResize: true,
                spellcheck: false,
            },
        });

        const contextMenu = Menu.buildFromTemplate([
            { role: 'cut' },
            { role: 'copy' },
            { role: 'paste' },
            { role: 'selectAll' },
        ]);

        mainWindow.webContents.on('context-menu', (event, params) => {
            contextMenu.popup({
                window: mainWindow,
                x: params.x,
                y: params.y,
            });
        });

        mainWindow.setBackgroundColor('#1E1E1E');

        mainWindow.setAspectRatio(1818 / 1123);

        // Get the current language
        const currentLanguage = getLanguage();

        // Send the language to the renderer
        mainWindow.webContents.on('did-finish-load', () => {
            mainWindow.webContents.send('set-language', currentLanguage);
        });

        // Load intro page
        mainWindow.loadFile('./public/intro.html');

        // After intro, navigate to the main page or install page
        setTimeout(() => {
            if (mainWindow) {
                const nextPage = isGameExist()
                    ? './public/index.html'
                    : './public/install.html';
                mainWindow.webContents.send('start-fade-out', nextPage);
            }
        }, 4500);

        setupIpcHandlers(mainWindow);

        setMainWindow(mainWindow);

        mainWindow.on('close', async (event) => {
            const gameProcess = getGameProcess();
            
            // If the game is running, terminate it and perform necessary functions
            if (gameProcess) {
                try {
                    process.kill(gameProcess.pid, 'SIGTERM'); // Terminate csgo.exe
                    console.log(`Terminated csgo.exe with PID: ${gameProcess.pid}`);
                } catch (error) {
                    console.error('Failed to terminate csgo.exe:', error);
                }
        
                try {
                    stopRichPresence();
                } catch (error) {
                    console.error('Error stopping Rich Presence:', error);
                }
        
                try {
                    // Pass the correct array to deleteCopiedFiles
                    await deleteCopiedFiles(copiedFilesGameFilesPure);
                    console.log('Deleted copiedFilesGameFilesPure successfully.');
                } catch (error) {
                    console.error('Error deleting copiedFilesGameFilesPure:', error);
                    // Depending on your needs, you might notify the user or attempt recovery
                }
            } else {
                // If the game is already closed, just perform necessary actions
                try {
                    stopRichPresence();
                } catch (error) {
                    console.error('Error stopping Rich Presence:', error);
                }
        
                try {
                    // Pass the correct array to deleteCopiedFiles
                    await deleteCopiedFiles(copiedFilesGameFilesPure);
                    console.log('Deleted copiedFilesGameFilesPure successfully.');
                } catch (error) {
                    console.error('Error deleting copiedFilesGameFilesPure:', error);
                    // Depending on your needs, you might notify the user or attempt recovery
                }
            }
        
            // Now call the function to move the launcher and create a shortcut
            try {
                moveLauncherAndCreateShortcut();
            } catch (error) {
                console.error('Error moving launcher and creating shortcut:', error);
            }
        });
    } 

} 

module.exports = {
    createAppropriateWindow,
};
