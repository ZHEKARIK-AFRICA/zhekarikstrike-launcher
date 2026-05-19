// gameHandlers.js

const { ipcMain } = require('electron');
const path = require('path');
const fs = require('fs');
const { getGamePath } = require('../configManager');
const { readRevIni, updateRevIni } = require('../revIniManager');
const { spawn } = require('child_process');
const { startRichPresence, stopRichPresence } = require('../RichPresence');
const { formatErrorMessage } = require('../utils');
const { copyFilesAndTrack, deleteCopiedFiles, copiedFilesGameFilesPure, copiedFilesGameFiles } = require('../fileManager'); // Import from fileManager

let gameProcess = null; // This will hold the zhekarikstrike.exe process

function setupGameHandlers(mainWindow) {
    ipcMain.handle('launch-game', async (event) => {
        try {
            // Check for administrative privileges
            const isElevated = (await import('is-elevated')).default;
            const elevated = await isElevated();
            if (!elevated) {
                throw new Error('Launcher must be run as administrator to launch the game.');
            }

            // Get game path from configuration
            const gamePath = getGamePath();
            if (!gamePath || !fs.existsSync(gamePath)) {
                throw new Error('Game path not set or does not exist');
            }

            // Path to the game executable
            const exePath = path.join(gamePath, 'RevLoader.exe');
            if (!fs.existsSync(exePath)) {
                throw new Error('Game executable not found');
            }

            // Delete previously copied files from game_files_pure (if any)
            try {
                await deleteCopiedFiles(copiedFilesGameFilesPure);
                console.log('Deleted previously copied files from game_files_pure');
            } catch (error) {
                console.error('Error deleting previously copied files from game_files_pure:', error);
                // For safety, abort the launch
                throw new Error('Failed to clean up previous game_files_pure files.');
            }

            // Copy files from game_files_pure before launching the game
            const sourceFolderPure = path.join(__dirname, '../../../public/game_files_pure');
            console.log(`Source folder (pure): ${sourceFolderPure}`);
            console.log(`Game path: ${gamePath}`);

            // Copy regular files (isTemporary = false)
            await copyFilesAndTrack(sourceFolderPure, gamePath, sourceFolderPure, copiedFilesGameFilesPure);
            console.log('Copied regular files from game_files_pure');

            // Copy temporary files (isTemporary = true)
            await copyFilesAndTrack(sourceFolderPure, gamePath, sourceFolderPure, copiedFilesGameFilesPure, true);
            console.log('Copied temporary files from game_files_pure');

            // Start Rich Presence
            startRichPresence();
            console.log('Rich Presence started');

            // Launch RevLoader.exe
            const revloaderProcess = spawn(exePath, [], {
                detached: false,
                stdio: 'ignore',
            });

            console.log(`RevLoader.exe launched with PID: ${revloaderProcess.pid}`);

            // Dynamically import ps-list to monitor processes
            const psListModule = await import('ps-list');
            const psList = psListModule.default;

            // Function to find zhekarikstrike.exe process
            const findCsgoProcess = async () => {
                const processes = await psList();
                // Filter processes by name (case-insensitive)
                const csgo = processes.find(p => p.name.toLowerCase() === 'zhekarikstrike.exe');
                return csgo;
            };

            // Wait until zhekarikstrike.exe appears in the process list
            const waitForCsgo = async () => {
                return new Promise((resolve, reject) => {
                    const interval = setInterval(async () => {
                        try {
                            const csgo = await findCsgoProcess();
                            if (csgo) {
                                clearInterval(interval);
                                resolve(csgo);
                            }
                        } catch (error) {
                            clearInterval(interval);
                            reject(error);
                        }
                    }, 1000); // Check every 1 second

                    // Timeout after 60 seconds
                    setTimeout(() => {
                        clearInterval(interval);
                        reject(new Error('Timed out waiting for zhekarikstrike.exe to start'));
                    }, 60000);
                });
            };

            try {
                const csgo = await waitForCsgo();
                gameProcess = csgo; // Save the zhekarikstrike.exe process
                console.log(`zhekarikstrike.exe found with PID: ${gameProcess.pid}`);

                // Handler for when zhekarikstrike.exe closes
                const monitorCsgo = async () => {
                    while (true) {
                        const processes = await psList();
                        const isCsgoRunning = processes.some(p => p.pid === gameProcess.pid);
                        if (!isCsgoRunning) {
                            console.log('zhekarikstrike.exe has been closed');

                            // Delete files copied from game_files_pure
                            try {
                                await deleteCopiedFiles(copiedFilesGameFilesPure);
                                console.log('Deleted files from game_files_pure');
                            } catch (error) {
                                console.error('Error deleting files from game_files_pure:', error);
                                // Depending on your needs, you might notify the user or attempt recovery
                            }

                            // Copy files from game_files back
                            try {
                                const sourceFolder = path.join(__dirname, '../../../public/game_files');
                                console.log(`Copying files from: ${sourceFolder}`);
                                await copyFilesAndTrack(sourceFolder, gamePath, sourceFolder, copiedFilesGameFiles);
                                console.log('Successfully copied files from game_files back');
                            } catch (error) {
                                console.error('Error copying files from game_files:', error);
                                // Notify the user or attempt recovery as needed
                            }

                            // Stop Rich Presence
                            try {
                                stopRichPresence();
                                console.log('Rich Presence stopped');
                            } catch (error) {
                                console.error('Error stopping Rich Presence:', error);
                            }

                            // Notify renderer process that the game has closed
                            event.sender.send('game-closed', 0);
                            gameProcess = null;
                            break;
                        }
                        await new Promise(resolve => setTimeout(resolve, 3000)); // Check every 3 seconds
                    }
                };

                monitorCsgo();

                return true;
            } catch (error) {
                console.error('Error finding zhekarikstrike.exe:', error);
                throw error;
            }

        } catch (error) {
            console.error('Error launching the game:', error);
            const userMessage = formatErrorMessage(error);
            event.sender.send('launch-error', userMessage);
        }
    });

    ipcMain.handle('get-game-data', async (event) => {
        try {
            const gamePath = getGamePath();
            if (!gamePath || !fs.existsSync(gamePath)) {
                throw new Error('Game path not set or does not exist');
            }

            const { playerName, clanTag, launchParams } = readRevIni(gamePath);
            return { nickname: playerName, clanTag, launchParams, gamePath };
        } catch (error) {
            console.error('Error retrieving game data:', error);
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
}

function getGameProcess() {
    return gameProcess;
}

module.exports = {
    setupGameHandlers,
    getGameProcess,
};
