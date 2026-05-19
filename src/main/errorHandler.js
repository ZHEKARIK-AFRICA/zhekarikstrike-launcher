// errorHandler.js

const { dialog } = require('electron');
const { formatErrorMessage } = require('./utils');

let mainWindow;
let updateWindow;

function setupErrorHandlers() {
    process.on('uncaughtException', (error) => {
        console.error('Uncaught Exception:', error);
        // Notify renderer processes about the error
        if (mainWindow && mainWindow.webContents) {
            mainWindow.webContents.send(
                'global-error',
                formatErrorMessage(error)
            );
        }
        if (updateWindow && updateWindow.webContents) {
            updateWindow.webContents.send(
                'global-error',
                formatErrorMessage(error)
            );
        }
        // Display a native dialog
        dialog.showErrorBox('Unexpected Error', formatErrorMessage(error));
    });

    process.on('unhandledRejection', (reason, promise) => {
        console.error('Unhandled Rejection at:', promise, 'reason:', reason);
        // Notify renderer processes about the error
        if (mainWindow && mainWindow.webContents) {
            mainWindow.webContents.send(
                'global-error',
                formatErrorMessage(reason)
            );
        }
        if (updateWindow && updateWindow.webContents) {
            updateWindow.webContents.send(
                'global-error',
                formatErrorMessage(reason)
            );
        }
        // Display a native dialog
        dialog.showErrorBox('Unexpected Error', formatErrorMessage(reason));
    });
}

function setMainWindow(win) {
    mainWindow = win;
}

function setUpdateWindow(win) {
    updateWindow = win;
}

module.exports = {
    setupErrorHandlers,
    setMainWindow,
    setUpdateWindow,
};