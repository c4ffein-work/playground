// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {CharityEvent} from "./CharityEvent.sol";

interface IEventRegistry {
    function isEvent(address) external view returns (bool);
}

/// @title ImpactRegistry
/// @notice Public reports of real-world results ("deeds"), each tied to a
/// paid milestone, pinning the report and photos by sha256.
///
/// Overlapping claims are not refused (a hard lock lets a fast copycat block
/// the real builder). They are recorded and flagged "contested" on both
/// sides, and the argument happens in the open (see Forum). A deed overlaps
/// another if it reuses a photo hash, or sits in the same ~11 m map cell or
/// one of its 8 neighbours, for the same category.
///
/// Sensitive deeds (shelters, clinics in conflict zones) are stored with
/// coordinates rounded to 0.1 degree (~11 km) and are never matched by place.
contract ImpactRegistry {
    int32 private constant CELL = 10; // 1e-4 degree in E5 units, ~11 m
    int32 private constant COARSE = 10_000; // 0.1 degree in E5 units, ~11 km

    struct Deed {
        address eventAddr;
        uint256 milestoneId;
        address reporter;
        string category; // "water-well", "school-roof", ...
        int32 latE5;
        int32 lonE5;
        bool sensitive;
        string reportURI;
        bytes32 reportHash; // sha256 of the report document
        uint64 reportedAt;
        uint32 conflicts; // how many other deeds overlap this one
    }

    IEventRegistry public immutable events;

    mapping(bytes32 => Deed) private _deeds;
    bytes32[] public deedIds;
    mapping(bytes32 => bytes32) public firstInCell; // cell key => first deed there
    mapping(bytes32 => bytes32) public firstWithPhoto; // photo sha256 => first deed using it
    mapping(address => mapping(uint256 => bytes32)) public milestoneClaim; // event => milestone => deed

    event DeedReported(
        bytes32 indexed deedId,
        address indexed eventAddr,
        uint256 indexed milestoneId,
        address reporter,
        string category,
        int32 latE5,
        int32 lonE5,
        bool sensitive,
        string reportURI,
        bytes32 reportHash
    );
    event PhotoRegistered(bytes32 indexed deedId, bytes32 indexed photoHash, string uri);
    /// `reason`: "place" or "photo".
    event DeedContested(bytes32 indexed deedId, bytes32 indexed otherDeedId, string reason);

    error NotAnEvent();
    error MilestoneNotPaid();
    error NotReporter();
    error MilestoneAlreadyClaimed(bytes32 deedId);
    error NoPhotos();
    error BadCoordinates();

    constructor(IEventRegistry events_) {
        events = events_;
    }

    function cellKey(string memory category, int32 latE5, int32 lonE5) public pure returns (bytes32) {
        return keccak256(abi.encode(category, _floorDiv(latE5, CELL), _floorDiv(lonE5, CELL)));
    }

    /// @notice File the public report for a paid milestone. Only the event
    /// organizer or the milestone's payee can do it.
    function reportDeed(
        address eventAddr,
        uint256 milestoneId,
        string calldata category,
        int32 latE5,
        int32 lonE5,
        bool sensitive,
        string calldata reportURI,
        bytes32 reportHash,
        bytes32[] calldata photoHashes,
        string[] calldata photoURIs
    ) external returns (bytes32 deedId) {
        if (!events.isEvent(eventAddr)) revert NotAnEvent();
        if (latE5 < -9_000_000 || latE5 > 9_000_000 || lonE5 < -18_000_000 || lonE5 > 18_000_000) {
            revert BadCoordinates();
        }
        if (photoHashes.length == 0 || photoHashes.length != photoURIs.length) revert NoPhotos();

        CharityEvent ev = CharityEvent(eventAddr);
        CharityEvent.Milestone memory m = ev.milestone(milestoneId);
        if (m.status != CharityEvent.Status.Released) revert MilestoneNotPaid();
        if (msg.sender != ev.organizer() && msg.sender != m.payee) revert NotReporter();
        bytes32 prev = milestoneClaim[eventAddr][milestoneId];
        if (prev != bytes32(0)) revert MilestoneAlreadyClaimed(prev);

        deedId = keccak256(abi.encode(eventAddr, milestoneId));
        milestoneClaim[eventAddr][milestoneId] = deedId;
        deedIds.push(deedId);

        if (sensitive) {
            latE5 = _floorDiv(latE5, COARSE) * COARSE;
            lonE5 = _floorDiv(lonE5, COARSE) * COARSE;
        }
        Deed storage d = _deeds[deedId];
        d.eventAddr = eventAddr;
        d.milestoneId = milestoneId;
        d.reporter = msg.sender;
        d.category = category;
        d.latE5 = latE5;
        d.lonE5 = lonE5;
        d.sensitive = sensitive;
        d.reportURI = reportURI;
        d.reportHash = reportHash;
        d.reportedAt = uint64(block.timestamp);
        emit DeedReported(deedId, eventAddr, milestoneId, msg.sender, category, latE5, lonE5, sensitive, reportURI, reportHash);

        if (!sensitive) _checkPlace(deedId, category, latE5, lonE5);
        for (uint256 i; i < photoHashes.length; ++i) {
            bytes32 h = photoHashes[i];
            bytes32 other = firstWithPhoto[h];
            if (other == bytes32(0)) firstWithPhoto[h] = deedId;
            else if (other != deedId) _contest(deedId, other, "photo");
            emit PhotoRegistered(deedId, h, photoURIs[i]);
        }
    }

    function deed(bytes32 deedId) external view returns (Deed memory) {
        return _deeds[deedId];
    }

    function deedCount() external view returns (uint256) {
        return deedIds.length;
    }

    function _checkPlace(bytes32 deedId, string calldata category, int32 latE5, int32 lonE5) private {
        int32 cLat = _floorDiv(latE5, CELL);
        int32 cLon = _floorDiv(lonE5, CELL);
        bytes32 own = keccak256(abi.encode(category, cLat, cLon));
        for (int32 dy = -1; dy <= 1; ++dy) {
            for (int32 dx = -1; dx <= 1; ++dx) {
                bytes32 other = firstInCell[keccak256(abi.encode(category, cLat + dy, cLon + dx))];
                if (other != bytes32(0)) _contest(deedId, other, "place");
            }
        }
        if (firstInCell[own] == bytes32(0)) firstInCell[own] = deedId;
    }

    function _contest(bytes32 a, bytes32 b, string memory reason) private {
        _deeds[a].conflicts += 1;
        _deeds[b].conflicts += 1;
        emit DeedContested(a, b, reason);
    }

    function _floorDiv(int32 a, int32 b) private pure returns (int32) {
        int32 q = a / b;
        if ((a % b != 0) && ((a < 0) != (b < 0))) q -= 1;
        return q;
    }
}
