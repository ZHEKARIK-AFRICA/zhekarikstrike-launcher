// main.js

const { app } = require('electron');
const { checkAndLaunch } = require('./updateManager');
const { setupErrorHandlers } = require('./errorHandler');


// Set up global error handlers
setupErrorHandlers();

// Start the application
app.whenReady().then(checkAndLaunch);

