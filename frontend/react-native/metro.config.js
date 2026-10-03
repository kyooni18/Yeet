const path = require('node:path')
const { getDefaultConfig } = require('expo/metro-config')
const config = getDefaultConfig(__dirname)
// Shared semantic client is source code outside the Expo package, not a runtime DSL.
config.watchFolders = [path.resolve(__dirname, '../shared')]
module.exports = config
