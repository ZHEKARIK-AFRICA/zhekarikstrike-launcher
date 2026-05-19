// ipcHandlers.js

const { setupWindowHandlers } = require('./handlers/windowHandlers');
const { setupGameHandlers } = require('./handlers/gameHandlers');
const { setupInstallHandlers } = require('./handlers/installHandlers');
const { setupLanguageHandlers } = require('./handlers/languageHandlers');
const { setupUtilityHandlers } = require('./handlers/utilityHandlers');

/**
 * Настраивает все IPC обработчики для главного процесса.
 * @param {BrowserWindow} mainWindow - Главное окно приложения.
 */
function setupIpcHandlers(mainWindow) {
    setupWindowHandlers(mainWindow);
    setupGameHandlers(mainWindow);
    setupInstallHandlers(mainWindow);
    setupLanguageHandlers(mainWindow);
    setupUtilityHandlers(mainWindow);
}

module.exports = {
    setupIpcHandlers,
};