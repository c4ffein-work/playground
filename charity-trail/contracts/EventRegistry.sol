// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {CharityEvent} from "./CharityEvent.sol";
import {IdentityRegistry} from "./IdentityRegistry.sol";

/// @title EventRegistry
/// @notice One public index of every charity event. It only creates events
/// whose organizer, arbiter and signers are verified organizations, with
/// every signer from a different org than the organizer and each other.
contract EventRegistry {
    IdentityRegistry public immutable identity;
    address[] public events;
    mapping(address => bool) public isEvent;

    event EventCreated(address indexed eventAddress, address indexed organizer, string name, string metadataURI);

    error Unverified(address who);
    error SameOrganization(address who);

    constructor(IdentityRegistry identity_) {
        identity = identity_;
    }

    function createEvent(
        string calldata name,
        string calldata metadataURI,
        address arbiter,
        address[] calldata attesters,
        uint32 threshold,
        uint64 challengeWindow,
        address[] calldata tokens
    ) external returns (address eventAddress) {
        _checkSigners(arbiter, attesters);

        CharityEvent e =
            new CharityEvent(name, metadataURI, msg.sender, arbiter, attesters, threshold, challengeWindow, tokens);
        eventAddress = address(e);
        events.push(eventAddress);
        isEvent[eventAddress] = true;
        emit EventCreated(eventAddress, msg.sender, name, metadataURI);
    }

    function _checkSigners(address arbiter, address[] calldata attesters) private view {
        bytes32 organizerOrg = identity.orgOf(msg.sender);
        if (organizerOrg == bytes32(0)) revert Unverified(msg.sender);
        if (identity.orgOf(arbiter) == bytes32(0)) revert Unverified(arbiter);

        bytes32[] memory seen = new bytes32[](attesters.length);
        for (uint256 i; i < attesters.length; ++i) {
            bytes32 org = identity.orgOf(attesters[i]);
            if (org == bytes32(0)) revert Unverified(attesters[i]);
            if (org == organizerOrg) revert SameOrganization(attesters[i]);
            for (uint256 j; j < i; ++j) {
                if (seen[j] == org) revert SameOrganization(attesters[i]);
            }
            seen[i] = org;
        }
    }

    function eventCount() external view returns (uint256) {
        return events.length;
    }
}
