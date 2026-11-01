// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title Forum
/// @notice Public, permanent discussion attached to anything in the system:
/// a deed, a milestone, an event or an organization. Each post can take a
/// stance (support / dispute) and pins its evidence by hash.
///
/// The contract stores no weights on purpose. Anyone can compute how much a
/// post counts from public data: here, the money its author (and whoever
/// delegated to them) actually gave through signed-off milestones. Readers
/// can apply other rules; the raw record stays the same.
contract Forum {
    int8 public constant DISPUTE = -1;
    int8 public constant COMMENT = 0;
    int8 public constant SUPPORT = 1;

    mapping(address => uint256) public nonces;
    mapping(address => address) public delegateOf;

    event Posted(
        bytes32 indexed id,
        address indexed author,
        bytes32 indexed subject,
        bytes32 replyTo,
        int8 stance,
        string uri,
        bytes32 contentHash
    );
    event Delegated(address indexed from, address indexed to);

    error BadStance();
    error SelfDelegation();

    /// @notice Subject ids: a deed id, `milestoneSubject(event, id)`, or
    /// `addressSubject(event or org)`.
    function post(bytes32 subject, bytes32 replyTo, int8 stance, string calldata uri, bytes32 contentHash)
        external
        returns (bytes32 id)
    {
        if (stance < DISPUTE || stance > SUPPORT) revert BadStance();
        id = keccak256(abi.encode(msg.sender, nonces[msg.sender]++));
        emit Posted(id, msg.sender, subject, replyTo, stance, uri, contentHash);
    }

    /// @notice Lend your weight to someone (an org, an evaluator, a friend).
    /// Posting yourself on a subject overrides it for that subject.
    /// Delegate to address(0) to take it back.
    function delegate(address to) external {
        if (to == msg.sender) revert SelfDelegation();
        delegateOf[msg.sender] = to;
        emit Delegated(msg.sender, to);
    }

    function milestoneSubject(address eventAddr, uint256 milestoneId) external pure returns (bytes32) {
        return keccak256(abi.encode(eventAddr, milestoneId));
    }

    function addressSubject(address a) external pure returns (bytes32) {
        return bytes32(uint256(uint160(a)));
    }
}
