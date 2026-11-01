// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {CharityEvent} from "./CharityEvent.sol";

/// @title EventRegistry
/// @notice One public index of every charity event, so explorers and
/// dashboards can discover them from a single address.
contract EventRegistry {
    address[] public events;

    event EventCreated(address indexed eventAddress, address indexed organizer, string name, string metadataURI);

    function createEvent(
        string calldata name,
        string calldata metadataURI,
        address arbiter,
        address[] calldata attesters,
        uint32 threshold,
        uint64 challengeWindow,
        address[] calldata tokens
    ) external returns (address eventAddress) {
        CharityEvent e =
            new CharityEvent(name, metadataURI, msg.sender, arbiter, attesters, threshold, challengeWindow, tokens);
        eventAddress = address(e);
        events.push(eventAddress);
        emit EventCreated(eventAddress, msg.sender, name, metadataURI);
    }

    function eventCount() external view returns (uint256) {
        return events.length;
    }
}
