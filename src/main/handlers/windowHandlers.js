// handlers/windowHandlers.js

const { ipcMain, BrowserWindow } = require('electron');


function setupWindowHandlers(mainWindow) {
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

    ipcMain.on('load-main-page', (event) => {
        if (mainWindow) {
            mainWindow.loadFile('./public/index.html');
        }
    });
}

module.exports = {
    setupWindowHandlers,
};