const { ipcMain } = require('electron');
const { t, loadTranslations, changeLanguage } = require('../../localization/localization');
const { getLanguage, setLanguage } = require('../configManager');


// Хэндлеры для управления языком
function setupLanguageHandlers(mainWindow) {
    // Получение текущего перевода по ключу
    ipcMain.handle('translate', (event, key) => {
        return t(key);  // Возвращаем перевод по ключу
    });

    // Установка языка и загрузка переводов
    ipcMain.handle('set-language', async (event, lang) => {
        await changeLanguage(lang);
        setLanguage(lang);
        mainWindow.webContents.send('language-changed', lang);  // Сообщаем рендереру, что язык изменён
    });

    ipcMain.handle('get-language', async () => {
        return getLanguage();
    });

    // Загрузка переводов для языка
    ipcMain.handle('load-translations', async (event, lang) => {
        await loadTranslations(lang);
        return 'Translations loaded';
    });
}

module.exports = {
    setupLanguageHandlers,
};