// src/utils.js (main process)
const { t } = require('../localization/localization'); // Импортируем функцию t

function formatErrorMessage(error) {
    if (error.message.includes('ENOENT')) {
        return t('errors.file_missing');
    } else if (error.message.includes('Game path not set')) {
        return t('errors.game_path_not_set');
    } else if (error.message.includes('Game executable not found')) {
        return t('errors.game_executable_not_found');
    } else if (error.message.includes('Launcher must be run as administrator')) {
        return t('errors.launcher_admin');
    } else {
        return t('errors.unknown_error');
    }
}

module.exports = {
    formatErrorMessage,
};