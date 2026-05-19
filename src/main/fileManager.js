// fileManager.js

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

// Import the list of temporary files
const temporaryFiles = require('./temporaryFiles');

// Arrays to store copied files from each folder
let copiedFilesGameFiles = [];
let copiedFilesGameFilesPure = [];

/**
 * Copies a single file from source to target using streams.
 * @param {string} source - Path to the source file.
 * @param {string} target - Path to the target file.
 * @returns {Promise<void>}
 */
function copyFile(source, target) {
    return new Promise((resolve, reject) => {
        const readStream = fs.createReadStream(source);
        const writeStream = fs.createWriteStream(target);

        readStream.on('error', reject);
        writeStream.on('error', reject);
        writeStream.on('close', resolve);

        readStream.pipe(writeStream);
    });
}

/**
 * Computes the SHA-256 checksum of a file.
 * @param {string} filePath - Path to the file.
 * @returns {Promise<string>} - The hexadecimal checksum string.
 */
function getFileChecksum(filePath) {
    return new Promise((resolve, reject) => {
        const hash = crypto.createHash('sha256');
        const stream = fs.createReadStream(filePath);
        stream.on('error', reject);
        stream.on('data', chunk => hash.update(chunk));
        stream.on('end', () => resolve(hash.digest('hex')));
    });
}

/**
 * Checks if a file is locked by attempting to open it exclusively.
 * @param {string} filePath - Path to the file.
 * @returns {Promise<boolean>} - True if locked, false otherwise.
 */
async function isFileLocked(filePath) {
    return new Promise((resolve) => {
        fs.access(filePath, fs.constants.F_OK, (err) => {
            if (err) {
                // File does not exist, so it's not locked
                console.log(`File does not exist and is not locked: ${filePath}`);
                resolve(false);
            } else {
                const stream = fs.createWriteStream(filePath, { flags: 'r+' });
                stream.on('open', () => {
                    stream.close();
                    console.log(`File is not locked: ${filePath}`);
                    resolve(false); // File is not locked
                });
                stream.on('error', () => {
                    console.log(`File is locked: ${filePath}`);
                    resolve(true); // File is locked
                });
            }
        });
    });
}

/**
 * Retries an asynchronous operation with exponential backoff.
 * @param {Function} operation - The asynchronous operation to retry.
 * @param {number} retries - Number of retry attempts.
 * @param {number} delay - Initial delay in milliseconds.
 * @param {number} [backoffFactor=2] - Multiplier for the delay.
 * @returns {Promise<any>}
 */
async function retryOperation(operation, retries, delay, backoffFactor = 2) {
    for (let attempt = 1; attempt <= retries; attempt++) {
        try {
            return await operation();
        } catch (error) {
            if (attempt === retries) {
                return;
            }
            const currentDelay = delay * Math.pow(backoffFactor, attempt - 1);
            console.warn(`Operation failed on attempt ${attempt}. Retrying in ${currentDelay}ms...`);
            await new Promise(resolve => setTimeout(resolve, currentDelay));
        }
    }
}

/**
 * Recursively copies files from source to target, overwriting existing files.
 * Verifies integrity using checksum and checks for file locks.
 * @param {string} source - Source directory path.
 * @param {string} target - Target directory path.
 * @param {string} rootSource - Root source directory path for relative path calculation.
 * @param {Array} copiedFilesArray - Array to track copied files.
 * @param {boolean} isTemporary - Indicates if the current copy operation is for temporary files.
 * @returns {Promise<void>}
 */
async function copyFilesAndTrack(source, target, rootSource, copiedFilesArray, isTemporary = false) {
    console.log(`Copying from folder: ${source} to folder: ${target}`);

    const files = fs.readdirSync(source);

    for (const file of files) {
        const currentSource = path.join(source, file);
        const relativePath = path.relative(rootSource, currentSource);
        const currentTarget = path.join(target, relativePath);

        console.log(`Processing file or folder: ${currentSource}`);

        if (fs.lstatSync(currentSource).isDirectory()) {
            if (!fs.existsSync(currentTarget)) {
                console.log(`Creating directory: ${currentTarget}`);
                fs.mkdirSync(currentTarget, { recursive: true });
            }
            await copyFilesAndTrack(currentSource, target, rootSource, copiedFilesArray, isTemporary);
        } else {
            try {
                // Check if the target file is locked
                const locked = await isFileLocked(currentTarget);
                if (locked) {
                    throw new Error(`File is locked and cannot be overwritten: ${currentTarget}`);
                }

                console.log(`Copying file to: ${currentTarget}`);
                await retryOperation(() => copyFile(currentSource, currentTarget), 3, 2000); // Retry up to 3 times with 2s delay

                // Verify file integrity
                const [sourceChecksum, targetChecksum] = await Promise.all([
                    getFileChecksum(currentSource),
                    getFileChecksum(currentTarget)
                ]);

                console.log(`Source Checksum (${path.basename(currentSource)}): ${sourceChecksum}`);
                console.log(`Target Checksum (${path.basename(currentTarget)}): ${targetChecksum}`);

                if (sourceChecksum !== targetChecksum) {
                    throw new Error(`Checksum mismatch for file: ${currentTarget}`);
                }

                console.log(`Successfully copied and verified: ${currentTarget}`);
                copiedFilesArray.push(currentTarget); // Add file to the list

                // If the file is temporary, schedule its replacement after 5 seconds
                if (isTemporary && temporaryFiles.includes(relativePath)) {
                    console.log(`Scheduling replacement for temporary file: ${currentTarget}`);
                    setTimeout(async () => {
                        try {
                            // Delete the temporary file
                            if (fs.existsSync(currentTarget)) {
                                await deleteCopiedFiles([currentTarget]);
                                console.log(`Deleted temporary file: ${currentTarget}`);
                            }

                            // Define source and target for replacement
                            const replacementSource = path.join(
                                rootSource.replace('game_files_pure', 'game_files'),
                                relativePath
                              );
                            const replacementTarget = currentTarget;

                            if (!fs.existsSync(replacementSource)) {
                                throw new Error(`Replacement file does not exist: ${replacementSource}`);
                            }

                            // Copy the replacement file
                            await copyFile(replacementSource, replacementTarget);
                            console.log(`Replaced temporary file with: ${replacementTarget}`);

                        } catch (error) {
                            console.error(`Error replacing temporary file ${currentTarget}:`, error);
                        }
                    }, 25000); // 5000 milliseconds = 5 seconds
                }

            } catch (error) {
                console.error(`Error copying file from ${currentSource} to ${currentTarget}:`, error);
            }
        }
    }
}

/**
 * Deletes a list of files after checking for locks.
 * @param {Array} copiedFilesArray - Array of file paths to delete.
 * @returns {Promise<void>}
 */
async function deleteCopiedFiles(copiedFilesArray) {
    // Defensive programming: Ensure copiedFilesArray is an array
    if (!Array.isArray(copiedFilesArray)) {
        console.error(`deleteCopiedFiles expected an array, but received:`, copiedFilesArray);
        throw new TypeError('deleteCopiedFiles expects an array of file paths.');
    }

    console.log(`deleteCopiedFiles called with ${copiedFilesArray.length} files to delete.`);

    for (const filePath of copiedFilesArray) {
        if (fs.existsSync(filePath)) {
            try {
                // Check if the file is locked before attempting to delete
                const locked = await isFileLocked(filePath);
                if (locked) {
                    throw new Error(`File is locked and cannot be deleted: ${filePath}`);
                }

                console.log(`Deleting file: ${filePath}`);
                fs.unlinkSync(filePath);
                console.log(`Successfully deleted: ${filePath}`);
            } catch (error) {
                console.error(`Error deleting file ${filePath}:`, error);
            }
        } else {
            console.warn(`File does not exist, skipping deletion: ${filePath}`);
        }
    }
    copiedFilesArray.length = 0; // Clear the array
    console.log(`All specified files have been deleted and the array has been cleared.`);
}

module.exports = {
    copyFilesAndTrack,
    deleteCopiedFiles,
    copiedFilesGameFiles,
    copiedFilesGameFilesPure,
};
