// handlers/utilityHandlers.js

const { ipcMain, dialog, shell } = require('electron');

function setupUtilityHandlers(mainWindow) {
    // Открытие внешней ссылки в браузере по умолчанию
    ipcMain.on('open-external', (event, url) => {
        shell.openExternal(url).catch((err) => {
            console.error(`Failed to open URL: ${url}`, err);
        });
    });

    // Открытие диалога выбора папки
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
}

module.exports = {
    setupUtilityHandlers,
};