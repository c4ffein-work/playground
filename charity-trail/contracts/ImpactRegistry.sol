// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {CharityEvent} from "./CharityEvent.sol";
import {IdentityRegistry} from "./IdentityRegistry.sol";

interface IEventRegistry {
    function isEvent(address) external view returns (bool);
}

/// @title ImpactRegistry
/// @notice Public reports of real-world results ("deeds"), each tied to a paid
/// milestone. Stops the same good deed from being claimed twice:
///   - a deed sits on a ~11 m map cell for its category; a new claim is refused
///     if that cell or any of its 8 neighbours already holds one;
///   - every photo hash can back one deed only;
///   - a milestone can back one deed only.
/// Anyone can then confirm or dispute a deed on-site. Votes from verified
/// organizations are counted separately.
contract ImpactRegistry {
    /// Coordinates are degrees * 1e5. A cell is 1e-4 degrees (~11 m).
    int32 private constant CELL = 10;

    struct Deed {
        address eventAddr;
        uint256 milestoneId;
        address reporter;
        string category; // "water-well", "school-roof", ...
        int32 latE5;
        int32 lonE5;
        string reportURI;
        bytes32 reportHash; // sha256 of the report document
        uint64 reportedAt;
        uint32 confirmations;
        uint32 verifiedConfirmations;
        uint32 disputes;
    }

    IEventRegistry public immutable events;
    IdentityRegistry public immutable identity;

    mapping(bytes32 => Deed) private _deeds;
    bytes32[] public deedIds;
    mapping(bytes32 => bytes32) public cellClaim; // cell key => deed id
    mapping(bytes32 => bytes32) public photoClaim; // photo sha256 => deed id
    mapping(address => mapping(uint256 => bytes32)) public milestoneClaim; // event => milestone => deed id
    mapping(bytes32 => mapping(address => bool)) public hasVoted;

    event DeedReported(
        bytes32 indexed deedId,
        address indexed eventAddr,
        uint256 indexed milestoneId,
        address reporter,
        string category,
        int32 latE5,
        int32 lonE5,
        string reportURI,
        bytes32 reportHash
    );
    event PhotoRegistered(bytes32 indexed deedId, bytes32 indexed photoHash, string uri);
    event DeedVoted(bytes32 indexed deedId, address indexed voter, bool confirms, bool verifiedVoter, string noteURI);

    error NotAnEvent();
    error MilestoneNotPaid();
    error NotReporter();
    error MilestoneAlreadyClaimed(bytes32 deedId);
    error DeedAlreadyClaimed(bytes32 deedId);
    error PhotoAlreadyUsed(bytes32 photoHash, bytes32 deedId);
    error NoPhotos();
    error BadCoordinates();
    error UnknownDeed();
    error AlreadyVoted();
    error CannotVoteOwnDeed();

    constructor(IEventRegistry events_, IdentityRegistry identity_) {
        events = events_;
        identity = identity_;
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
        _claimCells(deedId, category, latE5, lonE5);

        milestoneClaim[eventAddr][milestoneId] = deedId;
        deedIds.push(deedId);
        Deed storage d = _deeds[deedId];
        d.eventAddr = eventAddr;
        d.milestoneId = milestoneId;
        d.reporter = msg.sender;
        d.category = category;
        d.latE5 = latE5;
        d.lonE5 = lonE5;
        d.reportURI = reportURI;
        d.reportHash = reportHash;
        d.reportedAt = uint64(block.timestamp);
        emit DeedReported(deedId, eventAddr, milestoneId, msg.sender, category, latE5, lonE5, reportURI, reportHash);

        for (uint256 i; i < photoHashes.length; ++i) {
            bytes32 h = photoHashes[i];
            bytes32 owner = photoClaim[h];
            if (owner != bytes32(0)) revert PhotoAlreadyUsed(h, owner);
            photoClaim[h] = deedId;
            emit PhotoRegistered(deedId, h, photoURIs[i]);
        }
    }

    /// @notice Confirm ("I visited, the well works") or dispute a deed.
    function vote(bytes32 deedId, bool confirms, string calldata noteURI) external {
        Deed storage d = _deeds[deedId];
        if (d.reporter == address(0)) revert UnknownDeed();
        if (msg.sender == d.reporter) revert CannotVoteOwnDeed();
        if (hasVoted[deedId][msg.sender]) revert AlreadyVoted();
        hasVoted[deedId][msg.sender] = true;

        bool verified = identity.isVerified(msg.sender);
        if (confirms) {
            d.confirmations += 1;
            if (verified) d.verifiedConfirmations += 1;
        } else {
            d.disputes += 1;
        }
        emit DeedVoted(deedId, msg.sender, confirms, verified, noteURI);
    }

    function deed(bytes32 deedId) external view returns (Deed memory) {
        return _deeds[deedId];
    }

    function deedCount() external view returns (uint256) {
        return deedIds.length;
    }

    function _claimCells(bytes32 deedId, string calldata category, int32 latE5, int32 lonE5) private {
        int32 cLat = _floorDiv(latE5, CELL);
        int32 cLon = _floorDiv(lonE5, CELL);
        for (int32 dy = -1; dy <= 1; ++dy) {
            for (int32 dx = -1; dx <= 1; ++dx) {
                bytes32 existing = cellClaim[keccak256(abi.encode(category, cLat + dy, cLon + dx))];
                if (existing != bytes32(0)) revert DeedAlreadyClaimed(existing);
            }
        }
        cellClaim[keccak256(abi.encode(category, cLat, cLon))] = deedId;
    }

    function _floorDiv(int32 a, int32 b) private pure returns (int32) {
        int32 q = a / b;
        if ((a % b != 0) && ((a < 0) != (b < 0))) q -= 1;
        return q;
    }
}
