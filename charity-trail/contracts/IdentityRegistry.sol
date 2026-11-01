// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title IdentityRegistry
/// @notice Links wallets to verified organizations. Trusted issuers (charity
/// registers, auditors, vLEI issuers...) vouch that "wallet W belongs to org O".
/// Several wallets can belong to one org (same `orgId`), which lets the event
/// registry check that signers really are *different* organizations.
contract IdentityRegistry {
    struct Credential {
        bytes32 orgId; // e.g. keccak256("FR-RNA:W751234567")
        string name; // display name
        string uri; // public proof: registry page, vLEI, DNS TXT record...
        address issuer;
        uint64 issuedAt;
        bool revoked;
    }

    struct Issuer {
        bool active;
        string name;
    }

    /// In production: a governance multisig, not one key.
    address public owner;
    mapping(address => Issuer) public issuers;
    mapping(address => Credential) private _credentials;

    event IssuerSet(address indexed issuer, bool active, string name);
    event CredentialIssued(address indexed subject, bytes32 indexed orgId, string name, string uri, address indexed issuer);
    event CredentialRevoked(address indexed subject, address indexed by);
    event OwnershipTransferred(address indexed from, address indexed to);

    error NotOwner();
    error NotIssuer();
    error NotAllowed();
    error BadInput();

    constructor() {
        owner = msg.sender;
        emit OwnershipTransferred(address(0), msg.sender);
    }

    function transferOwnership(address to) external {
        if (msg.sender != owner) revert NotOwner();
        if (to == address(0)) revert BadInput();
        emit OwnershipTransferred(owner, to);
        owner = to;
    }

    function setIssuer(address issuer, bool active, string calldata name) external {
        if (msg.sender != owner) revert NotOwner();
        if (issuer == address(0)) revert BadInput();
        issuers[issuer] = Issuer(active, name);
        emit IssuerSet(issuer, active, name);
    }

    function issue(address subject, bytes32 orgId, string calldata name, string calldata uri) external {
        if (!issuers[msg.sender].active) revert NotIssuer();
        if (subject == address(0) || orgId == bytes32(0)) revert BadInput();
        _credentials[subject] = Credential(orgId, name, uri, msg.sender, uint64(block.timestamp), false);
        emit CredentialIssued(subject, orgId, name, uri, msg.sender);
    }

    /// @notice The issuer that vouched, or the owner, can withdraw a credential.
    function revoke(address subject) external {
        Credential storage c = _credentials[subject];
        if (msg.sender != c.issuer && msg.sender != owner) revert NotAllowed();
        c.revoked = true;
        emit CredentialRevoked(subject, msg.sender);
    }

    /// @notice Verified = credential exists, not revoked, issuer still trusted.
    function isVerified(address subject) public view returns (bool) {
        Credential storage c = _credentials[subject];
        return c.issuer != address(0) && !c.revoked && issuers[c.issuer].active;
    }

    /// @notice Org of a verified wallet, or zero.
    function orgOf(address subject) external view returns (bytes32) {
        return isVerified(subject) ? _credentials[subject].orgId : bytes32(0);
    }

    function credentialOf(address subject) external view returns (Credential memory) {
        return _credentials[subject];
    }
}
