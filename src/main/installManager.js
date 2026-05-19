// installManager.js

const path = require('path');
const axios = require('axios');
const yauzl = require('yauzl');
const fs = require('fs');
const crypto = require('crypto');
const async = require('async');
const { pipeline } = require('stream');
const { promisify } = require('util');
const streamPipeline = promisify(pipeline);
const checkDiskSpace = require('check-disk-space').default; // Import check-disk-space

const SERVER_URL = 'http://80.85.247.83:8000'; // Replace with your server URL
const DOWNLOAD_URL = 'http://80.85.247.83:80';
const ARCHIVE_URL = `${DOWNLOAD_URL}/download_game_archive`; // Archive download URL
// const ARCHIVE_URL = `https://download947.mediafire.com/eoybpxzcs01g6PcoEhcRwWtNExtOJwuynnoZyg3edOFE3yiWcuQG7a126wdpVrDsyMWVcQ_QEzFVLo5VzaV_PIXf2dx7pw4K25r0_8-BhKfzArlaaWFA9Jc58hVX-D6NPxxnAgIfh-zAXq1bDeeCkKSmYwm82wSAkbQ_prCR65EB/rsr4i5qgpzbvoah/zhekarik_client.zip`; // Alternative archive download URL

const { setGamePath, setGameVersion, getGamePath, getGameVersion } = require('./configManager'); // Import configManager functions

function delay(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

/**
 * Function to check available disk space on the target path.
 * @param {string} targetPath - The path where files will be downloaded or extracted.
 * @param {number} requiredSpace - The required space in bytes.
 * @returns {Promise<boolean>} - Returns true if sufficient space is available, else false.
 */
async function hasSufficientDiskSpace(targetPath, requiredSpace) {
    try {
        const diskSpace = await checkDiskSpace(targetPath);
        console.log(`Available disk space: ${diskSpace.free} bytes. Required: ${requiredSpace} bytes.`);
        return diskSpace.free >= requiredSpace;
    } catch (error) {
        console.error('Error checking disk space:', error);
        // If unable to check disk space, assume insufficient space to prevent potential issues
        return false;
    }
}

/**
 * Function to download a file with retry logic and progress tracking.
 * Immediate failure on fatal errors like ENOSPC.
 */
async function downloadFile(fileUrl, savePath, progressCallback, abortSignal, maxRetries = 3, retryDelay = 2000) {
    let attempt = 0;

    while (attempt <= maxRetries) {
        try {
            // Ensure the directory exists
            const directory = path.dirname(savePath);
            if (!fs.existsSync(directory)) {
                console.log(`Directory does not exist. Creating directory: ${directory}`);
                fs.mkdirSync(directory, { recursive: true });
            }

            // Initialize the download
            let response;
            try {
                response = await axios({
                    url: fileUrl,
                    method: 'GET',
                    responseType: 'stream',
                    signal: abortSignal, // Pass the abort signal
                    validateStatus: (status) => {
                        // Accept only status codes in the 200 range
                        return status >= 200 && status < 300;
                    },
                });
            } catch (error) {
                if (axios.isCancel(error)) {
                    console.warn(`Download canceled for: ${fileUrl}`);
                    throw new Error('Download canceled by the user.');
                } else {
                    console.error(`Failed to initiate download for: ${fileUrl}`);
                    console.error(`Error: ${error.message}`);
                    throw error;
                }
            }

            // Check for content-length header
            const contentLength = response.headers['content-length'];
            if (!contentLength) {
                console.warn(`No 'content-length' header found for: ${fileUrl}. Progress tracking may be inaccurate.`);
            }

            const totalSize = contentLength ? parseInt(contentLength, 10) : null;
            let downloadedSize = 0;
            const startTime = Date.now();

            // Wrap the response stream to track progress
            const progressStream = new require('stream').Transform({
                transform(chunk, encoding, callback) {
                    downloadedSize += chunk.length;
                    if (totalSize) {
                        const progress = (downloadedSize / totalSize) * 100;

                        // Calculate download speed and estimated time remaining
                        const elapsedTime = (Date.now() - startTime) / 1000; // in seconds
                        const downloadSpeed = downloadedSize / elapsedTime; // bytes per second
                        const timeRemaining = downloadSpeed > 0 ? (totalSize - downloadedSize) / downloadSpeed : 0; // in seconds

                        if (progressCallback) {
                            progressCallback(progress, 'download', timeRemaining);
                        }
                    } else {
                        // If content-length is not available
                        console.log(`WE DON'T KNOW FILE SIZE. Downloading ${savePath}: ${downloadedSize} bytes downloaded.`);
                        if (progressCallback) {
                            // Optionally, call progressCallback with partial data or skip it
                            progressCallback(null, 'download', null);
                        }
                    }
                    this.push(chunk);
                    callback();
                }
            });

            const writer = fs.createWriteStream(savePath);

            try {
                await streamPipeline(response.data, progressStream, writer);
                // If download is successful, break out of the retry loop
                console.log(`Successfully downloaded: ${fileUrl}`);
                break;
            } catch (error) {
                if (axios.isCancel(error)) {
                    console.warn(`Download canceled for: ${fileUrl}`);
                    throw new Error('Download canceled by the user.');
                } else {
                    console.error(`Error while downloading ${fileUrl}: ${error.message}`);
                    throw error;
                }
            }
        } catch (error) {
            // Check for fatal errors (e.g., ENOSPC)
            if (error.code === 'ENOSPC') {
                console.error(`Fatal error encountered (ENOSPC) for: ${fileUrl}. Aborting download.`);
                throw error; // Immediate failure without retry
            }

            if (axios.isCancel(error) || error.message === 'Download canceled by the user.') {
                // Do not retry if the error was due to cancellation
                throw error;
            }

            attempt += 1;
            console.error(`Attempt ${attempt} failed for: ${fileUrl}`);

            if (attempt > maxRetries) {
                console.error(`Max retries exceeded for: ${fileUrl}`);
                throw error; // Throw error after all retries fail
            }

            console.log(`Retrying download for: ${fileUrl} in ${retryDelay}ms...`);
            await delay(retryDelay); // Wait before retrying
        }
    }
}

/**
 * Modified downloadFileWithInstance to include immediate failure on fatal errors.
 */
async function downloadFileWithInstance(axiosInstance, fileUrl, savePath, progressCallback, abortSignal, maxRetries = 3, retryDelay = 2000) {
    let attempt = 0;

    while (attempt <= maxRetries) {
        try {
            // Ensure the directory exists
            const directory = path.dirname(savePath);
            if (!fs.existsSync(directory)) {
                console.log(`Directory does not exist. Creating directory: ${directory}`);
                fs.mkdirSync(directory, { recursive: true });
            }

            // Initialize the download
            let response;
            try {
                response = await axiosInstance({
                    url: fileUrl,
                    method: 'GET',
                    responseType: 'stream',
                    signal: abortSignal,
                    validateStatus: (status) => status >= 200 && status < 300,
                });
            } catch (error) {
                if (axios.isCancel(error)) {
                    console.warn(`Download canceled for: ${fileUrl}`);
                    throw new Error('Download canceled by the user.');
                } else {
                    console.error(`Failed to initiate download for: ${fileUrl}`);
                    console.error(`Error: ${error.message}`);
                    throw error;
                }
            }

            // Check for content-length header
            const contentLength = response.headers['content-length'];
            if (!contentLength) {
                console.warn(`No 'content-length' header found for: ${fileUrl}. Progress tracking may be inaccurate.`);
            }

            const totalSize = contentLength ? parseInt(contentLength, 10) : null;
            let downloadedSize = 0;
            const startTime = Date.now();

            // Wrap the response stream to track progress
            const progressStream = new require('stream').Transform({
                transform(chunk, encoding, callback) {
                    downloadedSize += chunk.length;
                    if (totalSize) {
                        const progress = (downloadedSize / totalSize) * 100;

                        // Calculate download speed and estimated time remaining
                        const elapsedTime = (Date.now() - startTime) / 1000; // in seconds
                        const downloadSpeed = downloadedSize / elapsedTime; // bytes per second
                        const timeRemaining = downloadSpeed > 0 ? (totalSize - downloadedSize) / downloadSpeed : 0; // in seconds

                        if (progressCallback) {
                            progressCallback(progress, 'download', timeRemaining);
                        }
                    } else {
                        // If content-length is not available
                        console.log(`WE DON'T KNOW FILE SIZE. Downloading ${savePath}: ${downloadedSize} bytes downloaded.`);
                        if (progressCallback) {
                            // Optionally, call progressCallback with partial data or skip it
                            progressCallback(null, 'download', null);
                        }
                    }
                    this.push(chunk);
                    callback();
                }
            });

            const writer = fs.createWriteStream(savePath);

            try {
                await streamPipeline(response.data, progressStream, writer);
                console.log(`Successfully downloaded: ${fileUrl}`);
                break; // Exit the retry loop on success
            } catch (error) {
                if (axios.isCancel(error)) {
                    console.warn(`Download canceled for: ${fileUrl}`);
                    throw new Error('Download canceled by the user.');
                } else {
                    console.error(`Error while downloading ${fileUrl}: ${error.message}`);
                    throw error;
                }
            }
        } catch (error) {
            // Check for fatal errors (e.g., ENOSPC)
            if (error.code === 'ENOSPC') {
                console.error(`Fatal error encountered (ENOSPC) for: ${fileUrl}. Aborting download.`);
                throw error; // Immediate failure without retry
            }

            if (axios.isCancel(error) || error.message === 'Download canceled by the user.') {
                // Do not retry if the error was due to cancellation
                throw error;
            }

            attempt += 1;
            console.error(`Attempt ${attempt} failed for: ${fileUrl}`);

            if (attempt > maxRetries) {
                console.error(`Max retries exceeded for: ${fileUrl}`);
                throw error; // Throw error after all retries fail
            }

            console.log(`Retrying download for: ${fileUrl} in ${retryDelay}ms...`);
            await delay(retryDelay); // Wait before retrying
        }
    }
}

/**
 * Function to extract an archive.
 */
function extractArchive(archivePath, extractToPath, progressCallback) {
    return new Promise((resolve, reject) => {
        yauzl.open(archivePath, { lazyEntries: true }, (err, zipfile) => {
            if (err) return reject(err);

            const totalFiles = zipfile.entryCount;
            let extractedFiles = 0;
            let hadError = false;
            const startTime = Date.now();

            zipfile.on('error', (err) => {
                hadError = true;
                reject(err);
            });

            zipfile.on('entry', (entry) => {
                if (hadError) return;

                const filePath = path.join(extractToPath, entry.fileName);

                if (/\/$/.test(entry.fileName)) {
                    // Directory entry
                    fs.promises.mkdir(filePath, { recursive: true })
                        .then(() => {
                            extractedFiles += 1;
                            const progress = (extractedFiles / totalFiles) * 100;

                            // Calculate remaining time
                            const elapsedTime = (Date.now() - startTime) / 1000;
                            const averageTimePerFile = elapsedTime / extractedFiles;
                            const filesRemaining = totalFiles - extractedFiles;
                            const timeRemaining = averageTimePerFile * filesRemaining;

                            progressCallback(progress, 'extract', timeRemaining);
                            zipfile.readEntry();
                        })
                        .catch(reject);
                } else {
                    // File entry
                    zipfile.openReadStream(entry, (err, readStream) => {
                        if (err) {
                            hadError = true;
                            return reject(err);
                        }

                        const writeStream = fs.createWriteStream(filePath);

                        readStream.on('error', reject);
                        writeStream.on('error', reject);

                        readStream.pipe(writeStream)
                            .on('finish', () => {
                                extractedFiles += 1;
                                const progress = (extractedFiles / totalFiles) * 100;

                                // Calculate remaining time
                                const elapsedTime = (Date.now() - startTime) / 1000;
                                const averageTimePerFile = elapsedTime / extractedFiles;
                                const filesRemaining = totalFiles - extractedFiles;
                                const timeRemaining = averageTimePerFile * filesRemaining;

                                progressCallback(progress, 'extract', timeRemaining);
                                zipfile.readEntry();
                            });
                    });
                }
            });

            zipfile.on('end', () => {
                if (!hadError) {
                    fs.unlinkSync(archivePath); // Delete the archive after extraction
                    resolve();
                }
            });

            zipfile.readEntry();
        });
    });
}

/**
 * Function to verify game files.
 */
async function verifyFiles(gamePath, progressCallback, abortSignal, checkAllFiles = true) {
    console.log('Starting verifyFiles with checkAllFiles =', checkAllFiles);
    let excludeFiles = [];
    try {
        const excludeResponse = await axios.get(`${SERVER_URL}/exclude_files`, { signal: abortSignal });
        console.log('Received exclude files response:', excludeResponse.data);
        excludeFiles = excludeResponse.data.files || [];
        console.log(`Excluding ${excludeFiles.length} files from full verification.`);
    } catch (err) {
        if (axios.isCancel(err)) {
            console.warn('Verification canceled by the user.');
            throw new Error('Verification canceled by the user.');
        }
        console.error('Failed to get exclude files list:', err);
    }

    try {
        // Choose endpoint based on checkAllFiles
        const endpoint = checkAllFiles ? `${SERVER_URL}/all_files` : `${SERVER_URL}/additional_check`;
        const response = await axios.get(endpoint, { signal: abortSignal });
        const serverFiles = response.data.files;

        if (!serverFiles || typeof serverFiles !== 'object') {
            console.error('serverFiles is not an object:', serverFiles);
            throw new Error('Invalid serverFiles format');
        }

        const excludeSet = new Set(excludeFiles);
        console.log('Exclude set:', excludeSet);

        const serverFileEntries = Object.entries(serverFiles);
        console.log('ServerFileEntries:', serverFiles);
        const totalFiles = serverFileEntries.length;
        
        let verifiedFiles = 0;
        let filesToDownload = [];
        const startTime = Date.now(); // For calculating remaining time
        console.log('game path:', gamePath);
        for (const [filePath, serverFileData] of serverFileEntries) {
            if (abortSignal && abortSignal.aborted) {
                throw new Error('Verification canceled by user');
            }

            const localFilePath = path.join(gamePath, filePath);

            let needsDownload = false;
            if (!fs.existsSync(localFilePath)) {
                needsDownload = true;
                console.log(`File ${filePath} does not exist locally; marked for download.`);
            } else if (!excludeSet.has(filePath)) {
                // If file is not in the exclude list, compare hashes
                try {
                    const localFileHash = await getFileHash(localFilePath);
                    if (localFileHash !== serverFileData.hash) {
                        needsDownload = true;
                        console.log(`File ${filePath} hash mismatch; local - ${localFileHash}, server - ${serverFileData.hash}; marked for download.`);
                    }
                } catch (err) {
                    console.error(`Error hashing file ${localFilePath}:`, err);
                    needsDownload = true;
                }
            } else {
                // File is in the exclude list, only check existence
                console.log(`File ${filePath} is in the exclude list; checking existence only.`);
                if (!fs.existsSync(localFilePath)) {
                    needsDownload = true;
                    console.log(`Excluded file ${filePath} does not exist locally; marked for download.`);
                }
            }

            if (needsDownload) {
                filesToDownload.push({ path: filePath });
            }

            verifiedFiles += 1;
            const progress = (verifiedFiles / totalFiles) * 100;

            // Calculate remaining time
            const elapsedTime = (Date.now() - startTime) / 1000; // in seconds
            const averageTimePerFile = elapsedTime / verifiedFiles;
            const filesRemaining = totalFiles - verifiedFiles;
            const timeRemaining = averageTimePerFile * filesRemaining;

            progressCallback(progress, 'verify', timeRemaining);
        }

        // Step 5: Download missing or invalid files
        if (filesToDownload.length > 0) {
            console.log(`Need to download ${filesToDownload.length} files.`);
            progressCallback(0, 'download', null);
            await downloadMissingFiles(filesToDownload, gamePath, progressCallback, abortSignal);
        } else {
            console.log('All files are good.');
        }

        // Step 6: After successful verification, update the game version from the server
        try {
            const versionResponse = await axios.get(`${SERVER_URL}/version_number`, { signal: abortSignal });
            const versionData = versionResponse.data;
            const serverGameVersion = versionData.game_version || '0.0.0';
            setGameVersion(serverGameVersion);
            console.log(`Game version updated to ${serverGameVersion}`);
        } catch (err) {
            if (axios.isCancel(err)) {
                console.warn('Verification canceled by the user.');
                throw new Error('Verification canceled by the user.');
            }
            console.error('Failed to get game version from server:', err);
        }
    } catch (err) {
        console.error('Failed to verify files:', err);
        throw err; // Throw the error so the handler can catch it
    }
} // End of verifyFiles function

/**
 * Function to update the game version.
 */
async function updateGameVersion(progressCallback, abortSignal) {
    const gamePath = getGamePath();
    const currentVersion = getGameVersion();

    // Step 1: Request updates since currentVersion
    let updates = {};
    try {
        const updatesResponse = await axios.get(`${SERVER_URL}/get_updates`, {
            params: {
                from_version: currentVersion
            },
            signal: abortSignal
        });
        console.log('Received updates response:', updatesResponse.data);
        updates = updatesResponse.data.updates || {};
        const updateCount = Object.keys(updates).length;
        console.log(`Updating ${updateCount} files from version ${currentVersion}.`);
    } catch (err) {
        if (axios.isCancel(err)) {
            console.warn('Update canceled by the user.');
            throw new Error('Update canceled by the user.');
        }
        console.error('Failed to get updates:', err);
        throw err;
    }

    // If no updates are available
    if (Object.keys(updates).length === 0) {
        console.log('No updates available.');
        progressCallback(100, 'update', 0);
        return;
    }

    // Step 2: Prepare list of files to download
    const serverFileEntries = Object.entries(updates);
    const totalFiles = serverFileEntries.length;
    let filesToDownload = [];
    const startTime = Date.now(); // For calculating remaining time

    for (const [filePath, serverFileData] of serverFileEntries) {
        if (abortSignal && abortSignal.aborted) {
            throw new Error('Update canceled by user');
        }

        const localFilePath = path.join(gamePath, filePath);

        let needsDownload = false;

        if (!fs.existsSync(localFilePath)) {
            needsDownload = true;
            console.log(`File ${filePath} does not exist locally; marked for download.`);
        } else {
            // Compare hash
            try {
                const localFileHash = await getFileHash(localFilePath);
                if (localFileHash !== serverFileData.hash) {
                    needsDownload = true;
                    console.log(`File ${filePath} hash mismatch; local - ${localFileHash}, server - ${serverFileData.hash}; marked for download.`);
                }
            } catch (err) {
                console.error(`Error hashing file ${localFilePath}:`, err);
                needsDownload = true;
            }
        }

        if (needsDownload) {
            filesToDownload.push({ path: filePath });
        }

        // Update progress based on verified files
        const verifiedFiles = filesToDownload.length;
        const progress = (verifiedFiles / totalFiles) * 100;

        // Calculate remaining time
        const elapsedTime = (Date.now() - startTime) / 1000; // in seconds
        const averageTimePerFile = elapsedTime / (verifiedFiles || 1);
        const filesRemaining = totalFiles - verifiedFiles;
        const timeRemaining = averageTimePerFile * filesRemaining;

        progressCallback(progress, 'verify', timeRemaining);
    }

    // Step 3: Download missing or invalid files
    if (filesToDownload.length > 0) {
        console.log(`Need to download ${filesToDownload.length} updated files.`);
        await downloadMissingFiles(filesToDownload, gamePath, progressCallback, abortSignal);
    } else {
        console.log('All files are up to date.');
    }

    // Step 4: Update the game version to the latest version
    try {
        const versionResponse = await axios.get(`${SERVER_URL}/version_number`, { signal: abortSignal });
        const versionData = versionResponse.data;
        const newGameVersion = versionData.game_version || '0.0.0';
        setGameVersion(newGameVersion);
        console.log(`Game version updated to ${newGameVersion}`);
        progressCallback(100, 'update', 0);
    } catch (err) {
        if (axios.isCancel(err)) {
            console.warn('Update canceled by the user.');
            throw new Error('Update canceled by the user.');
        }
        console.error('Failed to update game version:', err);
        throw err;
    }
}

const pLimit = require('p-limit');

/**
 * Function to download missing files with limited parallelism.
 * Incorporates immediate failure on fatal errors.
 */
const http = require('http');
const https = require('https');

async function downloadMissingFiles(files, gamePath, progressCallback, abortSignal) {
    const limit = pLimit(20); // Maximum 20 concurrent downloads
    const totalFiles = files.length;
    let completedFiles = 0;
    const startTime = Date.now();

    // Create an axios instance with Keep-Alive agent
    const axiosInstance = axios.create({
        baseURL: DOWNLOAD_URL,
        httpAgent: new http.Agent({ keepAlive: true }),  // For HTTP connections
        httpsAgent: new https.Agent({ keepAlive: true })  // For HTTPS connections, if needed
    });

    const downloadPromises = files.map(file => limit(async () => {
        if (abortSignal && abortSignal.aborted) {
            throw new Error('Download canceled by user');
        }

        const filePath = path.join(gamePath, file.path);
        const encodedFilePath = encodeURIComponent(file.path).replace(/%2F/g, '/'); // Encode but keep slashes
        const fileUrl = `/download/${encodedFilePath}`;

        try {
            // Estimate required space (optional)
            // You can fetch the file size from the server if available
            // For now, we'll proceed without pre-estimating

            // Use the axiosInstance with Keep-Alive agent
            await downloadFileWithInstance(axiosInstance, fileUrl, filePath, null, abortSignal);
            console.log('Finished download for file:', file.path);
        } catch (error) {
            console.error('Error downloading file:', file.path, error);
            throw error; // Re-throw if you want to stop all downloads on error
        }

        // Update completed files count
        completedFiles += 1;

        // Compute overall progress
        const totalProgress = (completedFiles / totalFiles) * 100;

        // Compute estimated time remaining
        const elapsedTime = (Date.now() - startTime) / 1000; // seconds
        const averageTimePerFile = elapsedTime / completedFiles;
        const filesRemaining = totalFiles - completedFiles;
        const overallTimeRemaining = averageTimePerFile * filesRemaining;

        console.log('Updating progress');
        // Call the progress callback with the updated total progress and phase
        progressCallback(totalProgress, 'download', overallTimeRemaining);
    }));

    await Promise.all(downloadPromises);
}

/**
 * Function to get the MD5 hash of a file.
 */
function getFileHash(filePath) {
    return new Promise((resolve, reject) => {
        const hash = crypto.createHash('md5');
        const stream = fs.createReadStream(filePath);

        stream.on('error', reject);
        stream.on('data', (chunk) => hash.update(chunk));
        stream.on('end', () => resolve(hash.digest('hex')));
    });
}

/**
 * Function to install the game.
 * Includes disk space checks before downloading and extracting.
 */
async function installGame(gamePath, progressCallback, abortSignal) {
    try {
        const archivePath = path.join(gamePath, 'client2709_2.zip');
        const loaderPath = path.join(gamePath, 'revLoader.exe');

        // Check for revLoader.exe
        if (fs.existsSync(loaderPath)) {
            console.log('revLoader.exe found, skipping download and extraction.');
        } else {
            // Step 1: Check disk space before downloading the archive
            // Assuming you have an estimate of the archive size. Replace 'ARCHIVE_SIZE_IN_BYTES' with actual value.
            const ARCHIVE_SIZE_IN_BYTES = 9216* 1024 * 1024; // Example: 500 MB
            const downloadTargetPath = gamePath;
            const hasSpace = await hasSufficientDiskSpace(downloadTargetPath, ARCHIVE_SIZE_IN_BYTES);
            if (!hasSpace) {
                throw new Error('Insufficient disk space to download the game archive.');
            }

            // Step 2: Download the archive
            progressCallback(0, 'download', null);
            await downloadFile(ARCHIVE_URL, archivePath, progressCallback, abortSignal);

            // Step 3: Check disk space before extracting the archive
            // Assuming extraction might require twice the size of the archive
            const EXTRACTION_SIZE_ESTIMATE = ARCHIVE_SIZE_IN_BYTES * 1.6;
            const extractTargetPath = gamePath;
            const hasSpaceForExtraction = await hasSufficientDiskSpace(extractTargetPath, EXTRACTION_SIZE_ESTIMATE);
            if (!hasSpaceForExtraction) {
                throw new Error('Insufficient disk space to extract the game archive.');
            }

            // Step 4: Extract the archive
            progressCallback(0, 'extract', null);
            await extractArchive(archivePath, gamePath, progressCallback); // Pass progressCallback

            // Step 5: Set game version after extraction
            try {
                setGameVersion('0.0.0');
                console.log('Game version set to 0.0.0 after extraction');
            } catch (err) {
                console.error('Failed to set game version after extraction:', err);
            }
        }

        // Step 6: Verify files
        progressCallback(0, 'verify', null);
        await verifyFiles(gamePath, progressCallback, abortSignal); // Pass abortSignal

        // Step 7: Save game path to configuration
        try {
            setGamePath(gamePath);
        } catch (err) {
            console.error('Failed to save game path to config:', err);
        }

        console.log('Game installed successfully!');
    } catch (error) {
        if (axios.isCancel(error) || (error.message && error.message.includes('canceled'))) {
            console.log('Installation canceled by user');
            progressCallback(0, 'cancel', 0);
        } else {
            console.error('Error during game installation:', error);
            progressCallback(0, 'error', 0);
        }
        throw error; // Re-throw the error for further handling
    }
}

module.exports = { installGame, verifyFiles, updateGameVersion };
