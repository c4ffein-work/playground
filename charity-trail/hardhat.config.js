require("@nomicfoundation/hardhat-toolbox");

const { PRIVATE_KEY, BASE_SEPOLIA_RPC = "https://sepolia.base.org" } = process.env;

module.exports = {
  solidity: { version: "0.8.24", settings: { optimizer: { enabled: true, runs: 200 }, viaIR: true } },
  networks: {
    baseSepolia: {
      url: BASE_SEPOLIA_RPC,
      chainId: 84532,
      accounts: PRIVATE_KEY ? [PRIVATE_KEY] : [],
    },
  },
};
