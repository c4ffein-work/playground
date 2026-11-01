// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

interface IEventIndex {
    function isEvent(address) external view returns (bool);
}

/// @title CharityEvent
/// @notice Escrow for one charity event (e.g. "Flood relief, Region X").
/// Donations in whitelisted stablecoins sit here. Money leaves only through
/// milestones, and a milestone pays out only when:
///   1. `threshold` independent attesters signed it off with evidence, and
///   2. a challenge window passed with no open dispute.
/// Every step emits an event, so the whole trail can be rebuilt from logs.
///
/// A milestone's payee can be another CharityEvent from the same registry
/// (e.g. a global NGO funding a local NGO's event). The money then lands in
/// that event's escrow as a donation, so it can be followed hop by hop.
contract CharityEvent is ReentrancyGuard {
    using SafeERC20 for IERC20;

    enum Status {
        Proposed, // waiting for attestations
        Approved, // threshold reached, challenge window running
        Disputed, // challenged, waiting for the arbiter
        Released, // paid out
        Rejected  // refused, funds back in the pool
    }

    struct Milestone {
        string descriptionURI; // what the money is for (IPFS doc, budget line...)
        address payee;
        address token;
        uint256 amount;
        Status status;
        uint32 approvals;
        uint64 challengeDeadline;
    }

    string public name;
    string public metadataURI;
    /// The registry that created this event (used to recognise child events).
    address public immutable registry;
    address public immutable organizer;
    address public immutable arbiter;
    uint32 public immutable threshold;
    uint64 public immutable challengeWindow;
    bool public cancelled;

    address[] private _attesters;
    mapping(address => bool) public isAttester;
    address[] private _tokens;
    mapping(address => bool) public isAcceptedToken;

    Milestone[] private _milestones;
    /// milestone => attester => evidence hash (zero = not attested)
    mapping(uint256 => mapping(address => bytes32)) public evidenceOf;

    // Per-token accounting.
    mapping(address => uint256) public totalDonated;
    mapping(address => uint256) public committed; // earmarked by live milestones (incl. released)
    mapping(address => uint256) public released;
    mapping(address => mapping(address => uint256)) public donatedBy; // token => donor => amount
    mapping(address => mapping(address => bool)) public refunded; // token => donor => done

    event Donated(address indexed donor, address indexed token, uint256 amount);
    event MilestoneProposed(
        uint256 indexed id, address indexed payee, address indexed token, uint256 amount, string descriptionURI
    );
    event Attested(uint256 indexed id, address indexed attester, bytes32 evidenceHash, string evidenceURI);
    event AttestationRevoked(uint256 indexed id, address indexed attester, string reasonURI);
    event MilestoneApproved(uint256 indexed id, uint64 challengeDeadline);
    event Challenged(uint256 indexed id, address indexed by, string reasonURI);
    event DisputeResolved(uint256 indexed id, bool upheld, string reasonURI);
    event Released(uint256 indexed id, address indexed payee, address indexed token, uint256 amount);
    event Cancelled(string reasonURI);
    event Refunded(address indexed donor, address indexed token, uint256 amount);
    event ReclaimedFromChild(uint256 indexed id, address indexed child, address indexed token, uint256 amount);

    error NotOrganizer();
    error NotArbiter();
    error NotAttester();
    error NotAllowedToChallenge();
    error ConflictOfInterest();
    error EventCancelled();
    error EventNotCancelled();
    error TokenNotAccepted();
    error ZeroAmount();
    error InsufficientUncommittedFunds();
    error BadStatus();
    error AlreadyAttested();
    error NotAttested();
    error ChallengeWindowOpen();
    error ChallengeWindowClosed();
    error NothingToRefund();
    error BadConfig();
    error NotAChildEvent();
    error ChildRejectsToken();

    modifier onlyOrganizer() {
        if (msg.sender != organizer) revert NotOrganizer();
        _;
    }

    modifier onlyArbiter() {
        if (msg.sender != arbiter) revert NotArbiter();
        _;
    }

    modifier notCancelled() {
        if (cancelled) revert EventCancelled();
        _;
    }

    constructor(
        string memory name_,
        string memory metadataURI_,
        address organizer_,
        address arbiter_,
        address[] memory attesters_,
        uint32 threshold_,
        uint64 challengeWindow_,
        address[] memory tokens_
    ) {
        if (organizer_ == address(0) || arbiter_ == address(0)) revert BadConfig();
        if (threshold_ == 0 || threshold_ > attesters_.length) revert BadConfig();
        if (tokens_.length == 0) revert BadConfig();

        name = name_;
        metadataURI = metadataURI_;
        registry = msg.sender;
        organizer = organizer_;
        arbiter = arbiter_;
        threshold = threshold_;
        challengeWindow = challengeWindow_;

        for (uint256 i; i < attesters_.length; ++i) {
            address a = attesters_[i];
            // The organizer can't vouch for their own spending.
            if (a == address(0) || a == organizer_ || isAttester[a]) revert BadConfig();
            isAttester[a] = true;
            _attesters.push(a);
        }
        for (uint256 i; i < tokens_.length; ++i) {
            address t = tokens_[i];
            if (t == address(0) || isAcceptedToken[t]) revert BadConfig();
            isAcceptedToken[t] = true;
            _tokens.push(t);
        }
    }

    // ---------------------------------------------------------------- donate

    function donate(address token, uint256 amount) external nonReentrant notCancelled {
        if (!isAcceptedToken[token]) revert TokenNotAccepted();
        if (amount == 0) revert ZeroAmount();
        IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        totalDonated[token] += amount;
        donatedBy[token][msg.sender] += amount;
        emit Donated(msg.sender, token, amount);
    }

    // ------------------------------------------------------------ milestones

    /// @notice Earmark donated funds for a purpose. Can't exceed what's uncommitted.
    function proposeMilestone(string calldata descriptionURI, address payee, address token, uint256 amount)
        external
        onlyOrganizer
        notCancelled
        returns (uint256 id)
    {
        if (!isAcceptedToken[token]) revert TokenNotAccepted();
        if (amount == 0) revert ZeroAmount();
        if (payee == address(0) || payee == address(this)) revert BadConfig();
        if (_isChildEvent(payee) && !CharityEvent(payee).isAcceptedToken(token)) revert ChildRejectsToken();
        if (committed[token] + amount > totalDonated[token]) revert InsufficientUncommittedFunds();

        committed[token] += amount;
        id = _milestones.length;
        _milestones.push(
            Milestone({
                descriptionURI: descriptionURI,
                payee: payee,
                token: token,
                amount: amount,
                status: Status.Proposed,
                approvals: 0,
                challengeDeadline: 0
            })
        );
        emit MilestoneProposed(id, payee, token, amount, descriptionURI);
    }

    /// @notice Sign off a milestone. `evidenceHash` pins the justification
    /// document (photos, invoices, field report) pointed to by `evidenceURI`.
    function attest(uint256 id, bytes32 evidenceHash, string calldata evidenceURI) external notCancelled {
        if (!isAttester[msg.sender]) revert NotAttester();
        Milestone storage m = _milestones[id];
        if (msg.sender == m.payee) revert ConflictOfInterest();
        // Nor can the organizer of a child event sign a grant into it.
        if (_isChildEvent(m.payee) && CharityEvent(m.payee).organizer() == msg.sender) revert ConflictOfInterest();
        if (m.status != Status.Proposed && m.status != Status.Approved) revert BadStatus();
        if (evidenceHash == bytes32(0)) revert ZeroAmount();
        if (evidenceOf[id][msg.sender] != bytes32(0)) revert AlreadyAttested();

        evidenceOf[id][msg.sender] = evidenceHash;
        m.approvals += 1;
        emit Attested(id, msg.sender, evidenceHash, evidenceURI);

        if (m.status == Status.Proposed && m.approvals >= threshold) {
            m.status = Status.Approved;
            m.challengeDeadline = uint64(block.timestamp) + challengeWindow;
            emit MilestoneApproved(id, m.challengeDeadline);
        }
    }

    /// @notice An attester changes their mind before the money moves.
    function revokeAttestation(uint256 id, string calldata reasonURI) external {
        Milestone storage m = _milestones[id];
        if (m.status != Status.Proposed && m.status != Status.Approved) revert BadStatus();
        if (evidenceOf[id][msg.sender] == bytes32(0)) revert NotAttested();

        delete evidenceOf[id][msg.sender];
        m.approvals -= 1;
        emit AttestationRevoked(id, msg.sender, reasonURI);

        if (m.status == Status.Approved && m.approvals < threshold) {
            m.status = Status.Proposed;
            m.challengeDeadline = 0;
        }
    }

    /// @notice Donors and attesters can freeze an approved milestone during
    /// its challenge window. The arbiter then decides.
    function challenge(uint256 id, string calldata reasonURI) external {
        Milestone storage m = _milestones[id];
        if (m.status != Status.Approved) revert BadStatus();
        if (block.timestamp >= m.challengeDeadline) revert ChallengeWindowClosed();
        if (!isAttester[msg.sender] && donatedBy[m.token][msg.sender] == 0) revert NotAllowedToChallenge();

        m.status = Status.Disputed;
        emit Challenged(id, msg.sender, reasonURI);
    }

    function resolveDispute(uint256 id, bool upheld, string calldata reasonURI) external onlyArbiter nonReentrant {
        Milestone storage m = _milestones[id];
        if (m.status != Status.Disputed) revert BadStatus();
        emit DisputeResolved(id, upheld, reasonURI);
        if (upheld && !cancelled) {
            _release(id, m);
        } else {
            m.status = Status.Rejected;
            committed[m.token] -= m.amount;
        }
    }

    /// @notice Anyone can trigger payout once the window closed undisputed.
    function release(uint256 id) external nonReentrant notCancelled {
        Milestone storage m = _milestones[id];
        if (m.status != Status.Approved) revert BadStatus();
        if (block.timestamp < m.challengeDeadline) revert ChallengeWindowOpen();
        _release(id, m);
    }

    function _release(uint256 id, Milestone storage m) private {
        m.status = Status.Released;
        released[m.token] += m.amount;
        if (_isChildEvent(m.payee)) {
            // Donate into the child's escrow so its books record the inflow.
            IERC20(m.token).forceApprove(m.payee, m.amount);
            CharityEvent(m.payee).donate(m.token, m.amount);
        } else {
            IERC20(m.token).safeTransfer(m.payee, m.amount);
        }
        emit Released(id, m.payee, m.token, m.amount);
    }

    // ------------------------------------------------------ cancel / refund

    /// @notice Stop the event. Unreleased money goes back to donors pro rata.
    function cancel(string calldata reasonURI) external onlyArbiter notCancelled {
        cancelled = true;
        emit Cancelled(reasonURI);
    }

    /// @notice If a funded child event gets cancelled, pull our share back.
    /// The money returns to this event's free pool. Anyone can call this.
    function reclaimFromChild(uint256 id) external nonReentrant {
        Milestone storage m = _milestones[id];
        if (m.status != Status.Released || !_isChildEvent(m.payee)) revert NotAChildEvent();
        IERC20 token = IERC20(m.token);
        uint256 before = token.balanceOf(address(this));
        CharityEvent(m.payee).claimRefund(m.token);
        uint256 got = token.balanceOf(address(this)) - before;
        released[m.token] -= got;
        committed[m.token] -= got;
        emit ReclaimedFromChild(id, m.payee, m.token, got);
    }

    function _isChildEvent(address a) private view returns (bool) {
        return registry.code.length > 0 && IEventIndex(registry).isEvent(a);
    }

    function refundableOf(address token, address donor) public view returns (uint256) {
        if (!cancelled || refunded[token][donor]) return 0;
        uint256 total = totalDonated[token];
        if (total == 0) return 0;
        return (donatedBy[token][donor] * (total - released[token])) / total;
    }

    function claimRefund(address token) external nonReentrant {
        if (!cancelled) revert EventNotCancelled();
        uint256 amount = refundableOf(token, msg.sender);
        if (amount == 0) revert NothingToRefund();
        refunded[token][msg.sender] = true;
        IERC20(token).safeTransfer(msg.sender, amount);
        emit Refunded(msg.sender, token, amount);
    }

    // ----------------------------------------------------------------- views

    function milestone(uint256 id) external view returns (Milestone memory) {
        return _milestones[id];
    }

    function milestoneCount() external view returns (uint256) {
        return _milestones.length;
    }

    function attesters() external view returns (address[] memory) {
        return _attesters;
    }

    function acceptedTokens() external view returns (address[] memory) {
        return _tokens;
    }
}
