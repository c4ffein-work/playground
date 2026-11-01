// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

interface IEventIndex {
    function isEvent(address) external view returns (bool);
}

/// @title CharityEvent
/// @notice Escrow for one charity event (e.g. "Clean water for Kayes").
/// Donations in whitelisted stablecoins sit here. Money leaves only through
/// milestones, and a milestone pays out as soon as `threshold` independent
/// signers have signed it off with evidence.
///
/// Nobody can freeze money. The check on bad behaviour is that everything is
/// public and donors' future money can go elsewhere (see GivingVaults).
///
/// A milestone's payee can be another CharityEvent from the same registry
/// (e.g. a global NGO funding a local NGO's event). The money then lands in
/// that event's escrow as a donation, so it can be followed hop by hop.
contract CharityEvent is ReentrancyGuard {
    using SafeERC20 for IERC20;

    enum Status {
        Proposed, // waiting for signatures
        Released, // signed off and paid
        Withdrawn // pulled by the organizer, funds back in the pool
    }

    struct Milestone {
        string descriptionURI; // what the money is for (budget line, quote...)
        address payee;
        address token;
        uint256 amount;
        Status status;
        uint32 approvals;
    }

    string public name;
    string public metadataURI;
    /// The registry that created this event (used to recognise child events).
    address public immutable registry;
    address public immutable organizer;
    uint32 public immutable threshold;
    bool public closed;

    address[] private _attesters;
    mapping(address => bool) public isAttester;
    address[] private _tokens;
    mapping(address => bool) public isAcceptedToken;

    Milestone[] private _milestones;
    /// milestone => attester => evidence hash (zero = not signed)
    mapping(uint256 => mapping(address => bytes32)) public evidenceOf;

    // Per-token accounting.
    mapping(address => uint256) public totalDonated;
    mapping(address => uint256) public committed; // earmarked by open + released milestones
    mapping(address => uint256) public released;
    mapping(address => mapping(address => uint256)) public donatedBy; // token => donor => amount
    mapping(address => mapping(address => bool)) public refunded; // token => donor => done

    event Donated(address indexed donor, address indexed token, uint256 amount, address indexed via);
    event MilestoneProposed(
        uint256 indexed id, address indexed payee, address indexed token, uint256 amount, string descriptionURI
    );
    event Attested(uint256 indexed id, address indexed attester, bytes32 evidenceHash, string evidenceURI);
    event MilestoneWithdrawn(uint256 indexed id, string reasonURI);
    event Released(uint256 indexed id, address indexed payee, address indexed token, uint256 amount);
    event Closed(string reasonURI);
    event Refunded(address indexed donor, address indexed token, uint256 amount);
    event ReclaimedFromChild(uint256 indexed id, address indexed child, address indexed token, uint256 amount);

    error NotOrganizer();
    error NotAttester();
    error ConflictOfInterest();
    error EventClosed();
    error EventNotClosed();
    error TokenNotAccepted();
    error ZeroAmount();
    error InsufficientUncommittedFunds();
    error BadStatus();
    error AlreadyAttested();
    error NothingToRefund();
    error BadConfig();
    error NotAChildEvent();
    error ChildRejectsToken();

    modifier onlyOrganizer() {
        if (msg.sender != organizer) revert NotOrganizer();
        _;
    }

    modifier notClosed() {
        if (closed) revert EventClosed();
        _;
    }

    constructor(
        string memory name_,
        string memory metadataURI_,
        address organizer_,
        address[] memory attesters_,
        uint32 threshold_,
        address[] memory tokens_
    ) {
        if (organizer_ == address(0)) revert BadConfig();
        if (threshold_ == 0 || threshold_ > attesters_.length) revert BadConfig();
        if (tokens_.length == 0) revert BadConfig();

        name = name_;
        metadataURI = metadataURI_;
        registry = msg.sender;
        organizer = organizer_;
        threshold = threshold_;

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

    function donate(address token, uint256 amount) external {
        _donate(msg.sender, token, amount);
    }

    /// @notice Pay from the caller, credit `donor`. Used by giving vaults so
    /// the real donor shows up in the books.
    function donateFor(address donor, address token, uint256 amount) external {
        if (donor == address(0)) revert BadConfig();
        _donate(donor, token, amount);
    }

    function _donate(address donor, address token, uint256 amount) private nonReentrant notClosed {
        if (!isAcceptedToken[token]) revert TokenNotAccepted();
        if (amount == 0) revert ZeroAmount();
        IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        totalDonated[token] += amount;
        donatedBy[token][donor] += amount;
        emit Donated(donor, token, amount, msg.sender);
    }

    // ------------------------------------------------------------ milestones

    /// @notice Earmark donated funds for a purpose. Can't exceed what's uncommitted.
    function proposeMilestone(string calldata descriptionURI, address payee, address token, uint256 amount)
        external
        onlyOrganizer
        notClosed
        returns (uint256 id)
    {
        if (!isAcceptedToken[token]) revert TokenNotAccepted();
        if (amount == 0) revert ZeroAmount();
        if (payee == address(0) || payee == address(this)) revert BadConfig();
        if (_isChildEvent(payee) && !CharityEvent(payee).isAcceptedToken(token)) revert ChildRejectsToken();
        if (committed[token] + amount > totalDonated[token]) revert InsufficientUncommittedFunds();

        committed[token] += amount;
        id = _milestones.length;
        _milestones.push(Milestone(descriptionURI, payee, token, amount, Status.Proposed, 0));
        emit MilestoneProposed(id, payee, token, amount, descriptionURI);
    }

    /// @notice The organizer pulls a milestone signers won't sign (e.g. after
    /// a public argument showed it was overpriced).
    function withdrawMilestone(uint256 id, string calldata reasonURI) external onlyOrganizer {
        Milestone storage m = _milestones[id];
        if (m.status != Status.Proposed) revert BadStatus();
        m.status = Status.Withdrawn;
        committed[m.token] -= m.amount;
        emit MilestoneWithdrawn(id, reasonURI);
    }

    /// @notice Sign off a milestone. `evidenceHash` pins the justification
    /// document. The payment goes out with the signature that reaches the
    /// threshold.
    function attest(uint256 id, bytes32 evidenceHash, string calldata evidenceURI) external nonReentrant notClosed {
        if (!isAttester[msg.sender]) revert NotAttester();
        Milestone storage m = _milestones[id];
        if (msg.sender == m.payee) revert ConflictOfInterest();
        // Nor can the organizer of a child event sign a grant into it.
        if (_isChildEvent(m.payee) && CharityEvent(m.payee).organizer() == msg.sender) revert ConflictOfInterest();
        if (m.status != Status.Proposed) revert BadStatus();
        if (evidenceHash == bytes32(0)) revert ZeroAmount();
        if (evidenceOf[id][msg.sender] != bytes32(0)) revert AlreadyAttested();

        evidenceOf[id][msg.sender] = evidenceHash;
        m.approvals += 1;
        emit Attested(id, msg.sender, evidenceHash, evidenceURI);

        if (m.approvals >= threshold) _release(id, m);
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

    // ------------------------------------------------------- close / refund

    /// @notice The organizer winds the event down. Unspent money goes back to
    /// donors pro rata; open milestones can no longer be paid.
    function close(string calldata reasonURI) external onlyOrganizer notClosed {
        closed = true;
        emit Closed(reasonURI);
    }

    function refundableOf(address token, address donor) public view returns (uint256) {
        if (!closed || refunded[token][donor]) return 0;
        uint256 total = totalDonated[token];
        if (total == 0) return 0;
        return (donatedBy[token][donor] * (total - released[token])) / total;
    }

    function claimRefund(address token) external nonReentrant {
        if (!closed) revert EventNotClosed();
        uint256 amount = refundableOf(token, msg.sender);
        if (amount == 0) revert NothingToRefund();
        refunded[token][msg.sender] = true;
        IERC20(token).safeTransfer(msg.sender, amount);
        emit Refunded(msg.sender, token, amount);
    }

    /// @notice If a funded child event closes, pull our share back into this
    /// event's pool. Anyone can call this.
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
