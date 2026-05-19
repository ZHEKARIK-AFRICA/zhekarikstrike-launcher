// updateManager.js

const { app } = require('electron');
const { createAppropriateWindow } = require('./windowManager');

const {
    checkForUpdates,
    downloadAndReplaceLauncher,
} = require('./launcherManager');

async function checkAndLaunch() {
    try {
        await createAppropriateWindow();
    } catch (error) {
        console.error('Error during update check or launch:', error);
    }
}

module.exports = {
    checkAndLaunch,
};