// Import 'app' from Electron
const { app } = require('electron');
const fs = require('fs');
const path = require('path');
const os = require('os');

const APP_NAME = 'ZHEKARIKSTRIKE';

// Function to get the configuration directory path
function getConfigDirectory() {
    return path.join(os.homedir(), 'AppData', 'Local', APP_NAME);
}

// Function to get the configuration file path
function getConfigFilePath() {
    const configDir = getConfigDirectory();
    if (!fs.existsSync(configDir)) {
        fs.mkdirSync(configDir, { recursive: true });
    }
    return path.join(configDir, 'config.json');
}

// Function to load the configuration
function loadConfig() {
    const configPath = getConfigFilePath();
    if (fs.existsSync(configPath)) {
        return JSON.parse(fs.readFileSync(configPath, 'utf8'));
    }
    return {};
}

// Function to save the configuration
function saveConfig(config) {
    const configPath = getConfigFilePath();
    fs.writeFileSync(configPath, JSON.stringify(config, null, 4));
}

// Function to get the game path from the configuration
function getGamePath() {
    const config = loadConfig();
    return config.gamePath || null;
}

// Function to set the game path in the configuration
function setGamePath(gamePath) {
    const config = loadConfig();
    config.gamePath = gamePath;
    saveConfig(config);
}

// **Functions for Language Settings**
// Function to detect system language
function getSystemLanguage() {
    // Attempt to use Intl API
    try {
        const lang = Intl.DateTimeFormat().resolvedOptions().locale;
        return lang.split('-')[0].toLowerCase(); // e.g., 'en-US' -> 'en'
    } catch (error) {
        console.warn('Intl API not available. Falling back to environment variables.');
    }

    // Fallback to environment variables
    const envLang = process.env.LANG || process.env.LC_ALL || process.env.LC_MESSAGES || 'en';
    return envLang.split('.')[0].split('_')[0].toLowerCase();
}

// Function to get the language from the configuration or system
function getLanguage() {
    const config = loadConfig();
    if (config.language) {
        return config.language;
    } else {
        // Get the system language using Electron's 'app' API
        const systemLanguage = app.getLocale().split('-')[0]; // Get the first part (e.g., 'en' from 'en-US')
        return systemLanguage || 'ru'; // Default to 'ru' if system language cannot be detected
    }
}

// Function to set the language in the configuration
function setLanguage(language) {
    const config = loadConfig();
    config.language = language;
    saveConfig(config);
    console.log(`Language set to: ${language}`);
}

// Functions for game version
function getGameVersion() {
    const config = loadConfig();
    return config.gameVersion || '0.0.0';
}

function setGameVersion(gameVersion) {
    const config = loadConfig();
    config.gameVersion = gameVersion;
    saveConfig(config);
}

function isGameExist() {
    const gamePath = getGamePath();
    if (!gamePath || !fs.existsSync(gamePath)) {
        return false;
    }

    const revLoaderPath = path.join(gamePath, 'RevLoader.exe');

    if (fs.existsSync(revLoaderPath)) {
        return true;
    }

    function getDirectorySize(directoryPath) {
        let totalSize = 0;

        function calculateDirectorySize(dirPath) {
            const files = fs.readdirSync(dirPath);
            for (const file of files) {
                const fullPath = path.join(dirPath, file);
                const stats = fs.statSync(fullPath);

                if (stats.isDirectory()) {
                    calculateDirectorySize(fullPath);
                } else {
                    totalSize += stats.size;
                }
            }
        }

        calculateDirectorySize(directoryPath);
        return totalSize;
    }

    const directorySize = getDirectorySize(gamePath);
    const sizeThreshold = 10 * 1024 * 1024 * 1024;

    return directorySize > sizeThreshold;
}

module.exports = {
    loadConfig,
    saveConfig,
    getConfigDirectory,
    getConfigFilePath,
    getGamePath,
    setGamePath,
    getGameVersion,
    setGameVersion,
    getLanguage,
    setLanguage,
    isGameExist
};